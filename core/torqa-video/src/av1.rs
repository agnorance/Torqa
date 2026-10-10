//! AV1 decoding with rav1d, the Rust port of dav1d (#39). FFmpeg's own AV1 decoder only drives
//! hardware decoders, which most Macs lack, so AV1 packets come here instead; FFmpeg still
//! reads the file and converts the pictures.
//!
//! rav1d 1.1 offers only dav1d's C interface, so this module is the crate's one place with
//! `unsafe` code: small wrappers whose safety rests on dav1d's documented contracts, each
//! noted where it is used.
#![allow(unsafe_code)] // FFI to rav1d's dav1d API, the only interface the crate publishes

use std::ffi::c_int;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use ffmpeg_next as ffmpeg;
use rav1d::Dav1dResult;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{
    DAV1D_PIXEL_LAYOUT_I400, DAV1D_PIXEL_LAYOUT_I422, DAV1D_PIXEL_LAYOUT_I444,
};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_flush,
    dav1d_get_picture, dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use crate::VideoError;

/// A dav1d decoder.
pub(crate) struct Av1 {
    context: Option<Dav1dContext>,
}

/// How a decoded picture's samples are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    I400,
    I420,
    I422,
    I444,
}

/// A decoded picture, released when dropped.
pub(crate) struct Picture {
    picture: Dav1dPicture,
}

/// What `rav1d` answered besides success: its queue is full or empty for now, or it failed.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    TryAgain,
    Failed,
}

/// `rav1d` answers like `dav1d`: 0, or an errno of the C library negated, which it does not
/// name. On Unix the platform reads such a number back; on Windows it would read a Windows
/// error code, another numbering altogether, under which "try again" was taken for a failure
/// and every AV1 video stalled there.
fn check(result: Dav1dResult) -> Result<(), Answer> {
    if result.0 >= 0 {
        Ok(())
    } else if try_again(-result.0) {
        Err(Answer::TryAgain)
    } else {
        Err(Answer::Failed)
    }
}

#[cfg(windows)]
fn try_again(errno: c_int) -> bool {
    // EAGAIN of Microsoft's C library, the one `rav1d` gets on Windows.
    const EAGAIN: c_int = 11;
    errno == EAGAIN
}

#[cfg(not(windows))]
fn try_again(errno: c_int) -> bool {
    std::io::Error::from_raw_os_error(errno).kind() == std::io::ErrorKind::WouldBlock
}

fn failed(what: &str) -> VideoError {
    VideoError::Av1(what.to_owned())
}

impl Av1 {
    /// A decoder using as many threads as dav1d sees fit.
    pub(crate) fn new() -> Result<Self, VideoError> {
        // dav1d's defaults decode several frames at once, on all cores: with a single frame in
        // flight a laptop decodes 720p slower than it plays. Pictures then come out a few
        // packets after their data went in, which `decode` and `pictures` handle.
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` writes a complete `Dav1dSettings` to its argument,
        // which may be uninitialised; afterwards it is initialised.
        let mut settings = unsafe {
            dav1d_default_settings(NonNull::from(&mut settings).cast());
            settings.assume_init()
        };
        let mut context: Option<Dav1dContext> = None;
        // SAFETY: both pointers come from live, exclusive references for the call; dav1d writes
        // the context to the first and only reads the second.
        let result = unsafe {
            dav1d_open(
                Some(NonNull::from(&mut context)),
                Some(NonNull::from(&mut settings)),
            )
        };
        check(result).map_err(|_| failed("cannot start the AV1 decoder"))?;
        Ok(Self { context })
    }

    /// Feeds one packet of AV1 data (copied) shown at `timestamp`, and adds the pictures ready
    /// to `out`.
    pub(crate) fn decode(
        &mut self,
        bytes: &[u8],
        timestamp: i64,
        out: &mut Vec<Picture>,
    ) -> Result<(), VideoError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut data = Dav1dData::default();
        // SAFETY: `data` is a live, exclusive `Dav1dData`; dav1d allocates `bytes.len()` bytes
        // for it and returns their start, or null on failure.
        let buffer = unsafe { dav1d_data_create(Some(NonNull::from(&mut data)), bytes.len()) };
        if buffer.is_null() {
            return Err(failed("out of memory for AV1 data"));
        }
        // SAFETY: `buffer` points to `bytes.len()` freshly allocated bytes owned by `data`,
        // which cannot overlap `bytes`.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        data.m.timestamp = timestamp;
        let result = loop {
            // SAFETY: the context is from `dav1d_open` and not closed; `data` is live and
            // exclusive, and dav1d takes from it what it consumes.
            let sent = unsafe { dav1d_send_data(self.context, Some(NonNull::from(&mut data))) };
            match check(sent) {
                Ok(()) if data.sz == 0 => break Ok(()),
                Ok(()) => {}
                // Its queue is full: take pictures out, then send the rest.
                Err(Answer::TryAgain) => {
                    if let Err(error) = self.pictures(out) {
                        break Err(error);
                    }
                }
                Err(_) => break Err(failed("cannot decode this AV1 video")),
            }
        };
        if data.sz > 0 {
            // SAFETY: `data` still holds what dav1d did not consume; this releases it once.
            unsafe { dav1d_data_unref(Some(NonNull::from(&mut data))) };
        }
        result?;
        self.pictures(out)
    }

    /// Adds the pictures ready to `out`, e.g. the last ones at the end of the stream.
    pub(crate) fn pictures(&mut self, out: &mut Vec<Picture>) -> Result<(), VideoError> {
        loop {
            let mut picture = Dav1dPicture::default();
            // SAFETY: the context is live; dav1d writes a picture to the exclusive `picture`.
            let result =
                unsafe { dav1d_get_picture(self.context, Some(NonNull::from(&mut picture))) };
            match check(result) {
                Ok(()) => out.push(Picture { picture }),
                Err(Answer::TryAgain) => return Ok(()),
                Err(_) => return Err(failed("cannot decode this AV1 video")),
            }
        }
    }

    /// Forgets everything sent, e.g. before a seek.
    pub(crate) fn flush(&mut self) {
        if let Some(context) = self.context {
            // SAFETY: the context is from `dav1d_open` and not closed.
            unsafe { dav1d_flush(context) };
        }
    }
}

impl Drop for Av1 {
    fn drop(&mut self) {
        // SAFETY: closes the context from `dav1d_open` once and sets it to `None`.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.context))) };
    }
}

impl Picture {
    pub(crate) fn width(&self) -> u32 {
        u32::try_from(self.picture.p.w).unwrap_or(0)
    }

    pub(crate) fn height(&self) -> u32 {
        u32::try_from(self.picture.p.h).unwrap_or(0)
    }

    /// Bits per sample: 8, 10 or 12.
    pub(crate) fn bits(&self) -> u32 {
        u32::try_from(self.picture.p.bpc).unwrap_or(8)
    }

    pub(crate) fn layout(&self) -> Layout {
        match self.picture.p.layout {
            DAV1D_PIXEL_LAYOUT_I400 => Layout::I400,
            DAV1D_PIXEL_LAYOUT_I422 => Layout::I422,
            DAV1D_PIXEL_LAYOUT_I444 => Layout::I444,
            // I420, by far the most common, and anything newer.
            _ => Layout::I420,
        }
    }

    /// The timestamp sent with the picture's data.
    pub(crate) fn timestamp(&self) -> i64 {
        self.picture.m.timestamp
    }

    /// Plane `index` (0 luma, 1 and 2 chroma) as `rows` rows of `row_bytes` each, with the
    /// distance between rows; `None` for planes the layout lacks.
    pub(crate) fn plane(
        &self,
        index: usize,
        rows: usize,
        row_bytes: usize,
    ) -> Option<(&[u8], usize)> {
        let start = self.picture.data.get(index).copied().flatten()?;
        let stride = usize::try_from(self.picture.stride[usize::from(index > 0)]).ok()?;
        if rows == 0 || row_bytes > stride {
            return None;
        }
        let len = stride * (rows - 1) + row_bytes;
        // SAFETY: dav1d's picture planes hold `rows` rows of at least `row_bytes` bytes,
        // `stride` apart, and live as long as the picture, which outlives the slice.
        Some((
            unsafe { std::slice::from_raw_parts(start.as_ptr().cast::<u8>(), len) },
            stride,
        ))
    }
}

impl Drop for Picture {
    fn drop(&mut self) {
        // SAFETY: releases the picture from `dav1d_get_picture` once.
        unsafe { dav1d_picture_unref(Some(NonNull::from(&mut self.picture))) };
    }
}

/// The AV1 configuration (sequence header) a container stores for the stream: the
/// `av1C` record's OBUs after its 4-byte header. Sent before the first packet, so decoding
/// can start at any keyframe.
pub(crate) fn config_obus(parameters: &ffmpeg::codec::Parameters) -> Vec<u8> {
    // SAFETY: FFmpeg's codec parameters own `extradata` of `extradata_size` bytes (or null),
    // alive as long as `parameters`, which outlives this copy.
    let extradata = unsafe {
        let raw = &*parameters.as_ptr();
        if raw.extradata.is_null() || raw.extradata_size <= 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(
            raw.extradata,
            usize::try_from(raw.extradata_size).unwrap_or(0),
        )
    };
    // An `av1C` record starts with its marker bit set; anything else is raw OBUs already.
    if extradata.first().is_some_and(|b| b & 0x80 != 0) && extradata.len() > 4 {
        extradata[4..].to_vec()
    } else {
        extradata.to_vec()
    }
}

/// The coded width and height FFmpeg read from the container for the stream.
pub(crate) fn size(parameters: &ffmpeg::codec::Parameters) -> (u32, u32) {
    // SAFETY: reads two plain integer fields of parameters FFmpeg filled in and owns.
    let raw = unsafe { &*parameters.as_ptr() };
    (
        u32::try_from(raw.width).unwrap_or(0),
        u32::try_from(raw.height).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_again_is_read_as_rav1d_means_it() {
        // The C library's EAGAIN: 35 on macOS, 11 on Linux and in Microsoft's; read as a
        // Windows error code, 11 would mean something else.
        let eagain = if cfg!(target_os = "macos") { 35 } else { 11 };
        assert_eq!(check(Dav1dResult(-eagain)), Err(Answer::TryAgain));
        assert_eq!(check(Dav1dResult(0)), Ok(()));
        assert_eq!(check(Dav1dResult(-22)), Err(Answer::Failed));
    }
}
