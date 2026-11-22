//! An Ogg Opus file through Symphonia and the station's codecs, as playback reads it.

use craft_audio::opus::Encoder;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

const RATE: usize = 48_000;
const FRAME: usize = Encoder::FRAME_SIZE;

/// One second of a 440 Hz tone on the left and 660 Hz on the right, interleaved.
fn tone() -> Vec<f32> {
    (0..RATE)
        .flat_map(|i| {
            let t = i as f32 / RATE as f32;
            let w = std::f32::consts::TAU;
            [0.4 * (w * 440.0 * t).sin(), 0.4 * (w * 660.0 * t).sin()]
        })
        .collect()
}

/// `pcm` as an Ogg Opus file (RFC 7845): the head, the tags, then 20 ms packets.
fn ogg_opus(pcm: &[f32]) -> Vec<u8> {
    let mut enc = Encoder::new(2, 128_000).expect("an encoder");
    let pre_skip = enc.pre_skip() as usize;
    // Silence after the audio flushes the encoder's delay and fills the last frame.
    let mut input = pcm.to_vec();
    let frames = pcm.len() / 2 + pre_skip;
    input.resize(frames.div_ceil(FRAME) * FRAME * 2, 0.0);
    let mut out = Vec::new();
    let mut w = PacketWriter::new(&mut out);
    w.write_packet(enc.opus_head(), 1, PacketWriteEndInfo::EndPage, 0)
        .expect("head");
    let mut tags = b"OpusTags".to_vec();
    tags.extend(4u32.to_le_bytes());
    tags.extend(b"test");
    tags.extend(0u32.to_le_bytes());
    w.write_packet(tags, 1, PacketWriteEndInfo::EndPage, 0)
        .expect("tags");
    let chunks: Vec<&[f32; FRAME * 2]> = input.as_chunks::<{ FRAME * 2 }>().0.iter().collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let packet = enc.encode_float(chunk.as_slice()).expect("a packet");
        let last = i + 1 == chunks.len();
        // The last page's granule is the real end, so the padding is trimmed (RFC 7845 4).
        let granule = if last {
            (pre_skip + pcm.len() / 2) as u64
        } else {
            ((i + 1) * FRAME) as u64
        };
        let end = if last {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        w.write_packet(packet, 1, end, granule).expect("a packet");
    }
    drop(w);
    out
}

/// Decodes `file` as playback does, to interleaved stereo.
fn play(file: Vec<u8>) -> Vec<f32> {
    let source = MediaSourceStream::new(Box::new(std::io::Cursor::new(file)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("opus");
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("an Ogg file");
    let track = format
        .default_track(TrackType::Audio)
        .expect("an audio track");
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .expect("audio")
        .clone();
    let mut decoder = radio_codecs::registry()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .expect("an Opus decoder");
    let (mut out, mut samples) = (Vec::new(), Vec::new());
    while let Ok(Some(packet)) = format.next_packet() {
        if packet.track_id != id {
            continue;
        }
        decoder
            .decode(&packet)
            .expect("the packet decodes")
            .copy_to_vec_interleaved(&mut samples);
        out.extend_from_slice(&samples);
    }
    out
}

#[test]
fn an_ogg_opus_file_plays_back_on_time_and_close_to_its_source() {
    let source = tone();
    let played = play(ogg_opus(&source));
    // The encoder's pre-skip is dropped: a lag would shift every sample of the tone.
    let compare = RATE / 2 * 2;
    let snr = |lag: usize| {
        let (mut signal, mut noise) = (0.0f64, 0.0f64);
        for (i, &x) in source[..compare].iter().enumerate() {
            let y = played.get(i + lag * 2).copied().unwrap_or(0.0);
            signal += f64::from(x * x);
            noise += f64::from((x - y) * (x - y));
        }
        10.0 * (signal / noise.max(1e-12)).log10()
    };
    let on_time = snr(0);
    assert!(
        on_time > 15.0,
        "the tone plays back clean and on time: {on_time:.1} dB"
    );
    assert!(
        played.len().abs_diff(source.len()) <= FRAME * 2,
        "the file plays for its own length: {} samples for {}",
        played.len(),
        source.len()
    );
}
