//! What an audio file is (spec §2.15; Lokal reads it with Symphonia, Stash with lofty, already in its core).
//!
//! lofty reads the stream's properties; containers it does not read (Matroska and WebM, FLAC in Ogg), or files it
//! cannot parse, are named from their first bytes, with the length unknown (0).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType};
use lofty::iff::wav::{WavFile, WavFormat};
use lofty::mp4::{Mp4Codec, Mp4File};
use lofty::probe::Probe;
use serde_json::{Value, json};

/// How much of the file's start is read to name it.
const HEAD: usize = 64 * 1024;

/// `{success: true, codec, audio_codec, bit_depth, bitDepth, sample_rate, sampleRate, channels, duration (s),
/// duration_ms, bitrate (bytes*8/duration), format}`; codec one of flac, alac, aac, mp3, opus, vorbis, wavpack,
/// pcm_s16le, pcm_s24le, pcm_s32le, pcm_f32le, unknown; format as Symphonia names it (flac, isomp4, ogg, wave, adts,
/// mkv, webm, wv, mp3). Fails `Unreadable audio: …`, or `The addon produced no audio stream`.
pub fn probe(path: &Path) -> Result<Value, String> {
    let unreadable = |e: std::io::Error| format!("Unreadable audio: {e}");
    let size = std::fs::metadata(path).map_err(unreadable)?.len();
    let head = read_head(path).map_err(unreadable)?;
    let sniffed = sniff(&head);
    if let Some(found) = sniffed.as_ref().filter(|found| found.beyond_lofty) {
        return Ok(summary(found.codec, found.format, &Stream::default(), size));
    }
    match read_with_lofty(path) {
        Ok((codec, format, stream)) => {
            if codec == "unknown" && stream.is_empty() {
                return Err("The addon produced no audio stream".into());
            }
            Ok(summary(codec, format, &stream, size))
        }
        Err(why) => match sniffed {
            Some(found) => Ok(summary(found.codec, found.format, &Stream::default(), size)),
            None => Err(format!("Unreadable audio: {why}")),
        },
    }
}

/// The extension a probed `format` is kept under (flac, mp3, m4a, ogg, wav, aac, webm, wv), when known.
pub fn extension_of(format: &str) -> Option<&'static str> {
    match format {
        "flac" => Some("flac"),
        "mp3" => Some("mp3"),
        "isomp4" => Some("m4a"),
        "ogg" => Some("ogg"),
        "wave" => Some("wav"),
        "adts" => Some("aac"),
        "mkv" | "webm" => Some("webm"),
        "wv" | "wavpack" => Some("wv"),
        _ => None,
    }
}

/// What lofty read of the audio stream (0 when unknown).
#[derive(Debug, Default, Clone, PartialEq)]
struct Stream {
    sample_rate: u32,
    bit_depth: u8,
    channels: u8,
    /// Seconds.
    duration: f64,
}

impl Stream {
    fn is_empty(&self) -> bool {
        self.sample_rate == 0 && self.channels == 0 && self.duration <= 0.0
    }
}

fn summary(codec: &str, format: &str, stream: &Stream, size: u64) -> Value {
    let duration = stream.duration;
    let bitrate = if duration > 0.0 {
        (size as f64 * 8.0 / duration) as u64
    } else {
        0
    };
    json!({
        "success": true,
        "codec": codec,
        "audio_codec": codec,
        "bit_depth": stream.bit_depth,
        "bitDepth": stream.bit_depth,
        "sample_rate": stream.sample_rate,
        "sampleRate": stream.sample_rate,
        "channels": stream.channels,
        "duration": duration,
        "duration_ms": (duration * 1000.0).round() as u64,
        "bitrate": bitrate,
        "format": format,
    })
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD);
    File::open(path)?.take(HEAD as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// The codec and container lofty finds by the file's content (never its name: add-ons write any audio to
/// `audio.flac`), and the stream's properties.
fn read_with_lofty(path: &Path) -> Result<(&'static str, &'static str, Stream), String> {
    let options = ParseOptions::new().read_tags(false).read_cover_art(false);
    let file = File::open(path).map_err(|e| e.to_string())?;
    let probe = Probe::new(BufReader::new(file))
        .options(options)
        .guess_file_type()
        .map_err(|e| e.to_string())?;
    let Some(kind) = probe.file_type() else {
        return Err("unknown format".into());
    };
    match kind {
        FileType::Mp4 => {
            let mut reader = probe.into_inner();
            let mp4 = Mp4File::read_from(&mut reader, options).map_err(|e| e.to_string())?;
            let found = mp4.properties();
            let codec = match found.codec() {
                Some(Mp4Codec::AAC) => "aac",
                Some(Mp4Codec::ALAC) => "alac",
                Some(Mp4Codec::MP3) => "mp3",
                Some(Mp4Codec::FLAC) => "flac",
                _ => "unknown",
            };
            let stream = Stream {
                sample_rate: found.sample_rate().unwrap_or(0),
                bit_depth: found.bit_depth().unwrap_or(0),
                channels: found.channels().unwrap_or(0),
                duration: found.duration().as_secs_f64(),
            };
            Ok((codec, "isomp4", stream))
        }
        FileType::Wav => {
            let mut reader = probe.into_inner();
            let wav = WavFile::read_from(&mut reader, options).map_err(|e| e.to_string())?;
            let found = wav.properties();
            let codec = match (found.format(), found.bit_depth()) {
                (WavFormat::PCM, 16) => "pcm_s16le",
                (WavFormat::PCM, 24) => "pcm_s24le",
                (WavFormat::PCM, 32) => "pcm_s32le",
                (WavFormat::IEEE_FLOAT, 32) => "pcm_f32le",
                _ => "unknown",
            };
            let stream = Stream {
                sample_rate: found.sample_rate(),
                bit_depth: found.bit_depth(),
                channels: found.channels(),
                duration: found.duration().as_secs_f64(),
            };
            Ok((codec, "wave", stream))
        }
        other => {
            let (codec, format) = names_of(other);
            let tagged = probe.read().map_err(|e| e.to_string())?;
            let found = tagged.properties();
            let stream = Stream {
                sample_rate: found.sample_rate().unwrap_or(0),
                bit_depth: found.bit_depth().unwrap_or(0),
                channels: found.channels().unwrap_or(0),
                duration: found.duration().as_secs_f64(),
            };
            Ok((codec, format, stream))
        }
    }
}

/// lofty's file type as Symphonia's codec and container names.
fn names_of(kind: FileType) -> (&'static str, &'static str) {
    match kind {
        FileType::Aac => ("aac", "adts"),
        FileType::Flac => ("flac", "flac"),
        FileType::Mpeg => ("mp3", "mp3"),
        FileType::Mp4 => ("unknown", "isomp4"),
        FileType::Opus => ("opus", "ogg"),
        FileType::Vorbis => ("vorbis", "ogg"),
        FileType::Speex => ("unknown", "ogg"),
        FileType::Wav => ("unknown", "wave"),
        FileType::WavPack => ("wavpack", "wv"),
        FileType::Aiff => ("unknown", "aiff"),
        FileType::Ape => ("unknown", "ape"),
        FileType::Mpc => ("unknown", "mpc"),
        FileType::Custom(name) => ("unknown", name),
        _ => ("unknown", "unknown"),
    }
}

/// A file named by its first bytes.
#[derive(Debug, Clone, PartialEq)]
struct Sniffed {
    codec: &'static str,
    format: &'static str,
    /// lofty cannot read it at all (so its guess, which may take a stray frame sync for MP3, is not asked).
    beyond_lofty: bool,
}

fn sniffed(codec: &'static str, format: &'static str) -> Option<Sniffed> {
    Some(Sniffed {
        codec,
        format,
        beyond_lofty: false,
    })
}

/// Names the file from its magic bytes: `fLaC`, `OggS` (+ `OpusHead` / `\x01vorbis` / `\x7FFLAC`), `ftyp`, EBML
/// (`1A 45 DF A3`), `ID3` or an MPEG frame sync, `RIFF`…`WAVE`, `wvpk`, an ADTS sync.
fn sniff(head: &[u8]) -> Option<Sniffed> {
    if head.starts_with(b"fLaC") {
        return sniffed("flac", "flac");
    }
    if head.starts_with(b"OggS") {
        return sniff_ogg(head);
    }
    if head.len() >= 8 && &head[4..8] == b"ftyp" {
        return sniffed(codec_named(head, &MP4_CODECS), "isomp4");
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let format = if contains(head, b"webm") {
            "webm"
        } else {
            "mkv"
        };
        return Some(Sniffed {
            codec: codec_named(head, &MATROSKA_CODECS),
            format,
            beyond_lofty: true,
        });
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WAVE" {
        return sniffed(wave_codec(head), "wave");
    }
    if head.starts_with(b"wvpk") {
        return sniffed("wavpack", "wv");
    }
    if head.starts_with(b"ID3") {
        return sniff_after_id3(head);
    }
    frame_sync(head)
}

fn sniff_ogg(head: &[u8]) -> Option<Sniffed> {
    // The first page's first packet follows its 27-byte header and segment table.
    let packet = head
        .get(26)
        .and_then(|&segments| head.get(27 + segments as usize..))
        .unwrap_or(&[]);
    if packet.starts_with(b"OpusHead") {
        return sniffed("opus", "ogg");
    }
    if packet.starts_with(b"\x01vorbis") {
        return sniffed("vorbis", "ogg");
    }
    if packet.starts_with(b"\x7FFLAC") {
        return Some(Sniffed {
            codec: "flac",
            format: "ogg",
            beyond_lofty: true,
        });
    }
    sniffed("unknown", "ogg")
}

/// What follows an ID3v2 tag: FLAC, or MPEG / ADTS frames (MP3 when unclear).
fn sniff_after_id3(head: &[u8]) -> Option<Sniffed> {
    if head.len() < 10 {
        return sniffed("mp3", "mp3");
    }
    let size = head[6..10]
        .iter()
        .fold(0usize, |size, &b| (size << 7) | usize::from(b & 0x7f));
    let footer = if head[5] & 0x10 != 0 { 10 } else { 0 };
    let rest = head.get(10 + size + footer..).unwrap_or(&[]);
    if rest.starts_with(b"fLaC") {
        return sniffed("flac", "flac");
    }
    frame_sync(rest).or_else(|| sniffed("mp3", "mp3"))
}

/// An ADTS (`FFF`, layer 0) or MPEG audio (11 sync bits, a layer) frame at the start.
fn frame_sync(head: &[u8]) -> Option<Sniffed> {
    match head {
        [0xFF, second, ..] if second & 0xF6 == 0xF0 => sniffed("aac", "adts"),
        [0xFF, second, ..] if second & 0xE0 == 0xE0 && second & 0x06 != 0 => sniffed("mp3", "mp3"),
        _ => None,
    }
}

/// The PCM kind a WAVE `fmt ` chunk names.
fn wave_codec(head: &[u8]) -> &'static str {
    let Some(at) = find(head, b"fmt ") else {
        return "unknown";
    };
    let u16_at = |i: usize| head.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let mut tag = u16_at(at + 8);
    let bits = u16_at(at + 22);
    if tag == Some(0xFFFE) {
        // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the real tag.
        tag = u16_at(at + 32);
    }
    match (tag, bits) {
        (Some(1), Some(16)) => "pcm_s16le",
        (Some(1), Some(24)) => "pcm_s24le",
        (Some(1), Some(32)) => "pcm_s32le",
        (Some(3), Some(32)) => "pcm_f32le",
        _ => "unknown",
    }
}

/// MP4 sample entries, in the order they are looked for.
const MP4_CODECS: [(&[u8], &str); 5] = [
    (b"alac", "alac"),
    (b"mp4a", "aac"),
    (b"fLaC", "flac"),
    (b"Opus", "opus"),
    (b".mp3", "mp3"),
];

/// Matroska codec ids.
const MATROSKA_CODECS: [(&[u8], &str); 6] = [
    (b"A_OPUS", "opus"),
    (b"A_VORBIS", "vorbis"),
    (b"A_FLAC", "flac"),
    (b"A_AAC", "aac"),
    (b"A_ALAC", "alac"),
    (b"A_MPEG/L3", "mp3"),
];

/// The codec of the first tag in `table` the head holds.
fn codec_named(head: &[u8], table: &[(&[u8], &'static str)]) -> &'static str {
    table
        .iter()
        .find(|(tag, _)| contains(head, tag))
        .map_or("unknown", |&(_, codec)| codec)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stash-probe-{}-{}",
            std::process::id(),
            crate::hex(&crate::random_bytes(4))
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// A WAVE file of `seconds` of 16-bit stereo silence at 44.1 kHz.
    fn wave(seconds: u32) -> Vec<u8> {
        let rate = 44_100u32;
        let data_len = rate * 4 * seconds;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&2u16.to_le_bytes()); // channels
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * 4).to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.resize(out.len() + data_len as usize, 0);
        out
    }

    #[test]
    fn reads_a_wave_file_whatever_its_name() {
        let path = temp_file("audio.flac", &wave(1));
        let info = probe(&path).unwrap();
        assert_eq!(info["success"], true);
        assert_eq!(info["codec"], "pcm_s16le");
        assert_eq!(info["audio_codec"], "pcm_s16le");
        assert_eq!(info["format"], "wave");
        assert_eq!(info["sample_rate"], 44_100);
        assert_eq!(info["sampleRate"], 44_100);
        assert_eq!(info["bit_depth"], 16);
        assert_eq!(info["channels"], 2);
        assert_eq!(info["duration_ms"], 1000);
        assert!(info["bitrate"].as_u64().unwrap() > 1_400_000);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn names_what_lofty_cannot_read_from_its_bytes() {
        let mut webm = vec![0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42, 0x82, 0x84];
        webm.extend_from_slice(b"webm....A_OPUS....");
        let path = temp_file("audio.flac", &webm);
        let info = probe(&path).unwrap();
        assert_eq!(info["codec"], "opus");
        assert_eq!(info["format"], "webm");
        assert_eq!(info["duration"], 0.0);
        assert_eq!(info["bitrate"], 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unreadable_files_say_so() {
        let path = temp_file("audio.flac", b"this is not audio at all, only words");
        let error = probe(&path).unwrap_err();
        assert!(error.starts_with("Unreadable audio: "), "{error}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(
            probe(Path::new("no/such/file.flac"))
                .unwrap_err()
                .starts_with("Unreadable audio: ")
        );
    }

    #[test]
    fn sniffs_magic_bytes() {
        let name = |head: &[u8]| sniff(head).map(|s| (s.codec, s.format));
        assert_eq!(name(b"fLaC\0\0\0\x22"), Some(("flac", "flac")));
        let mut ogg = b"OggS".to_vec();
        ogg.resize(26, 0);
        ogg.push(1); // one segment
        ogg.push(19);
        ogg.extend_from_slice(b"OpusHead\x01\x02");
        assert_eq!(name(&ogg), Some(("opus", "ogg")));
        ogg.truncate(28);
        ogg.extend_from_slice(b"\x01vorbis\0\0");
        assert_eq!(name(&ogg), Some(("vorbis", "ogg")));
        assert_eq!(
            name(b"\0\0\0\x20ftypM4A \0\0\0\0stsd....alac"),
            Some(("alac", "isomp4"))
        );
        assert_eq!(name(b"\0\0\0\x20ftypisom....mp4a"), Some(("aac", "isomp4")));
        assert_eq!(
            name(&[0x1A, 0x45, 0xDF, 0xA3, b'm', b'a', b't']),
            Some(("unknown", "mkv"))
        );
        assert_eq!(name(&[0xFF, 0xF1, 0x50, 0x80]), Some(("aac", "adts")));
        assert_eq!(name(&[0xFF, 0xFB, 0x90, 0x64]), Some(("mp3", "mp3")));
        assert_eq!(
            name(b"ID3\x04\0\0\0\0\0\x02\0\0fLaC"),
            Some(("flac", "flac"))
        );
        assert_eq!(
            name(b"ID3\x04\0\0\0\0\0\0\xFF\xFB\x90"),
            Some(("mp3", "mp3"))
        );
        assert_eq!(name(b"wvpk...."), Some(("wavpack", "wv")));
        assert_eq!(name(&wave(0)), Some(("pcm_s16le", "wave")));
        assert_eq!(name(b"plain text"), None);
    }

    #[test]
    fn formats_keep_their_extensions() {
        assert_eq!(extension_of("isomp4"), Some("m4a"));
        assert_eq!(extension_of("wave"), Some("wav"));
        assert_eq!(extension_of("adts"), Some("aac"));
        assert_eq!(extension_of("mkv"), Some("webm"));
        assert_eq!(extension_of("wavpack"), Some("wv"));
        assert_eq!(extension_of("aiff"), None);
    }
}
