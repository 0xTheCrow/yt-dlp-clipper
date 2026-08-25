//! Stream copying: adding pass-through output streams, writing copied packets,
//! and the two export paths that copy without decoding anything.

use anyhow::{anyhow, Result};
use std::sync::atomic::AtomicBool;
use ffmpeg_the_third as ffmpeg;
use ffmpeg::media::Type;

use super::{check_cancel, seek_to, write_header, ExportSpec};

/// Add an output stream carrying `params` through unchanged, for a stream copy.
/// The source codec tag is cleared so the output container assigns its own.
pub(super) fn add_copy_stream(
    octx: &mut ffmpeg::format::context::Output,
    params: ffmpeg::codec::Parameters,
) -> Result<usize> {
    let mut out = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
    out.set_parameters(params);
    unsafe {
        (*out.parameters().as_mut_ptr()).codec_tag = 0;
    }
    Ok(out.index())
}

/// Rescale `packet` from `in_tb` into output stream `out_index`'s time base and
/// write it. The muxer fixes each stream's time base in `write_header`, so
/// reading it here keeps every write on the value the muxer actually settled on.
pub(super) fn write_packet(
    packet: &mut ffmpeg::Packet,
    in_tb: ffmpeg::Rational,
    out_index: usize,
    octx: &mut ffmpeg::format::context::Output,
) -> Result<()> {
    let out_tb = octx.stream(out_index).unwrap().time_base();
    packet.set_stream(out_index);
    packet.rescale_ts(in_tb, out_tb);
    packet.write_interleaved(octx)?;
    Ok(())
}

/// Remux every stream of the whole file into the output container (copy only).
/// Unlike `transcode` this carries subtitle and data streams through too, since
/// nothing here has to be decoded.
pub(super) fn remux_copy(
    ictx: &mut ffmpeg::format::context::Input,
    spec: &ExportSpec,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut octx = ffmpeg::format::output(&spec.output)?;

    // input stream index -> (output index, input time base)
    let mut mapping: Vec<Option<(usize, ffmpeg::Rational)>> =
        vec![None; ictx.nb_streams() as usize];

    for in_stream in ictx.streams() {
        let in_index = in_stream.index();
        let in_tb = in_stream.time_base();
        let out_index = add_copy_stream(&mut octx, in_stream.parameters())?;
        mapping[in_index] = Some((out_index, in_tb));
    }

    write_header(&mut octx, spec.output.as_ref())?;

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(ictx).is_ok() {
        check_cancel(cancel)?;
        if let Some((out_index, in_tb)) = mapping[packet.stream()] {
            write_packet(&mut packet, in_tb, out_index, &mut octx)?;
        }
    }

    octx.write_trailer()?;
    Ok(())
}

/// Stream-copy the windowed audio into the output container, losslessly.
pub(super) fn export_audio_copy(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let mut octx = ffmpeg::format::output(&spec.output)?;

    let (in_index, in_tb, params) = {
        let in_stream = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| anyhow!("no audio stream found"))?;
        (in_stream.index(), in_stream.time_base(), in_stream.parameters())
    };
    let out_index = add_copy_stream(&mut octx, params)?;

    write_header(&mut octx, spec.output.as_ref())?;

    let start_ts = (spec.start_secs / f64::from(in_tb)).round() as i64;
    let end_ts = (spec.end_secs / f64::from(in_tb)).round() as i64;

    seek_to(&mut ictx, spec.start_secs)?;

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        check_cancel(cancel)?;
        if packet.stream() != in_index {
            continue;
        }
        let pts = packet.pts().unwrap_or(0);
        if pts < start_ts {
            continue;
        }
        if pts > end_ts {
            break;
        }
        rebase(&mut packet, start_ts);
        write_packet(&mut packet, in_tb, out_index, &mut octx)?;
    }

    octx.write_trailer()?;
    Ok(())
}

/// Shift a copied packet's timestamps so the clip starts at zero.
pub(super) fn rebase(packet: &mut ffmpeg::Packet, offset: i64) {
    packet.set_pts(packet.pts().map(|v| v - offset));
    packet.set_dts(packet.dts().map(|v| v - offset));
}
