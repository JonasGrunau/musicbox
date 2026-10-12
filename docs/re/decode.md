# Decoding and resampling

Status: **verified** on fixtures. All 169 oracle-a tracks decode to exactly the length rekordbox analysed: the PWV3 column count, the MP3 PVBR sample count and the FLAC PVB2 total all agree. On the click probe, every format's onset falls in the same 1/150 s column rekordbox drew in PWV3. The PVBR index tables of all 37 MP3s are reproduced byte-exact. Lossless paths (WAV, AIFF, FLAC, ALAC) are sample-identical to the WAV original.

The MP3 and AAC paths make the same library calls as rekordbox, in the same order, but they can't be compared sample by sample: rekordbox writes no PCM. The SRC is a literal port, including its FMA and factorial evaluation order, and it reproduces rekordbox's delay. Bit-exactness of its output will be confirmed through M6, by comparing waveforms and beats on resampled tracks.

Code: `crates/mb-audio` (decoders, SRC, timeline) and `crates/mb-player` (preview playback).

## 1. Reader selection
`rbxfrm::AudioReaderFactory::addDefaultFactories` @0x10284de70 registers these factories in this order:

1. Streaming
2. AudioPart
3. Wav
4. Aiff
5. Mp3
6. Mp4
7. Aac
8. Flac
9. Alac
10. Rbsc
11. Juce
12. iPod

`createReaderFor(path, true)` @0x10284e040 asks each factory in turn. A factory accepts by file extension, case-insensitive:

| Factory | Extensions |
|---|---|
| Wav | `.wav` |
| Aiff | `.aif` / `.aiff` |
| Mp3 | `.mp3` |
| Mp4 | `.m4a` / `.mp4` |
| Aac | `.aac` |
| Flac | `.flac` / `.fla` |
| Alac | `.m4a` / `.mp4` |

The candidate reader must then open, through `fileOpen` → `analyze` (vtable +0x38). If that fails, the next factory gets its turn. Consequences:
- **`.m4a`:** `FileReadMp4` needs an `esds` atom (AAC), so ALAC falls through to `FileReadAlac`.
- **`.wav`:** WAVE_FORMAT_EXTENSIBLE, float, 8-bit and 32-bit files fall through to `JuceAudioReaderWrapper`. `JuceAudioReaderFactory` uses JUCE's `registerBasicFormats`.
- **Ogg:** there is no Ogg factory in the list. `FileReadOgg` exists but isn't registered, so `.ogg` would go to JUCE.

mb-audio implements Wav, Aiff, Mp3, Mp4 (AAC), Alac, Flac and the JUCE WAV/AIFF fallback. It does not implement `.aac` (ADTS), Ogg, or JUCE's CoreAudio fallback.

## 2. Analysis rate, length and sample format
`analyzer::CBeatAnalyzeThread::analyze` @0x1001528c4 sets up the decode in four steps.

**Rate.** It builds `PcmReaderTemplate<Float2>(path, rate = 0, …, 0xd55, …)`, which decodes at the file's rate. It then reads `getFileSampleRate`:
- 44100 or 48000 is kept.
- Anything else rebuilds the reader with a target of **48000 if the rate is divisible by 8000**, else **44100**. The divisibility test is `ror32(rate·0x26e978d5, 6) ≤ 0x83126`, where 0x26e978d5 is the inverse of 125 modulo 2³².

Examples: 96k, 32k, 16k, 192k → 48k; 88.2k, 22.05k, 176.4k → 44.1k. **12k → 44.1k**, because 12000 is not a multiple of 8000. Code: `mb_audio::analysis_rate`.

**Converter.** `ReaderImpl<Float2>::setReader` @0x102855eec puts a `SampleRateConverter` between the reader and the template whenever the target differs from `reader->getSampleRate()`.

**Length.** Length L is the reader length (`FileReader` +0x40), or `SampleRateConverter::getLength` (+0x3c). The analysis loop reads positions `0 .. L−1`, so it **stops one sample short**.
- This matches every PWV3 count: columns = ⌈(L−1) / (rate/150)⌉ (`timeline::waveform_columns`).
- Example: 2,646,000 samples → 9000 columns. 2,647,296 → 9005.
- PWV5 equals PWV3 except on one AIFF (80,281 vs 80,280). That is a WaveCreator detail, not decoding.

**Analysis input is 16-bit.** `CBeatAnalyzeThread::getStreamAt` @0x100158f08 calls `PcmReaderTemplate<Float2>::getStreamAt(int, short*, short*, int)` @0x102853e3c. That call:
1. Multiplies each Float2 by 32768.
2. Clamps to [−32768, 32767]. NaN goes to −32768, following the `fcmp`/`b.le` path.
3. Truncates toward zero (`fcvtzs`).

The analysis then works on `(float)(short)`. The same arrays feed BALib, WaveCreator, the vocal detector and the loudness meter. Code: `mb_audio::analysis_i16`. For 16-bit sources this is the identity. For 24-bit, float and resampled audio it truncates.

**Float2 output.** Every reader returns `Float2 = {float L, float R}`. Mono is duplicated: R reads channel `channels−1`. Positions outside `[0, L)` read as 0.

## 3. Integer → float
| Reader | Conversion |
|---|---|
| `FileReadWav`, `FileReadAiff`, `FileReadAlac` | Sample bytes placed in the top of an `int32` (`s << (32−bits)`), `scvtf`, × 2⁻³¹. The scale is stored as 0x30000000 at +0x78. |
| `FileReadFlac` | `(float)s × 1/(float)ldexp(1, bits−1)` |
| `FileReadMp3`, `FileReadMp4` | 16-bit decoder output `(float)s × 2⁻¹⁵` |
| JUCE | Integers left-justified in `int32`, × `1.0f/0x7fffffff`. As a float constant that rounds to 2⁻³¹. Float data is passed through unchanged. |

All scales are powers of two, and ≤24-bit integers convert to float exactly. So every path gives **s / 2^(bits−1) exactly**, and lossless formats are interchangeable bit for bit.

## 4. PCM: WAV and AIFF

### `FileReadWav::analyze` @0x102838f6c
1. Requires RIFF/WAVE. Walks chunks from offset 12 in steps of `size + 8`, **with no pad byte for odd sizes**.
2. The first `fmt ` chunk is read as 18 bytes. It must be format tag 1 (PCM), 16 or 24 bits (`(bits|8) == 24`) and at most 2 channels. Otherwise the reader fails, which leads to the JUCE path.
3. The walk continues for `data`. Length = `data_size / (bytes·channels)`, unsigned.
4. A truncated data chunk reads as silence past the end of the file.
5. `fileRead` @0x102839988 reads 512 frames at a time.

### `FileReadAiff::analyze` @0x102810ad0
1. Requires FORM with AIFF or AIFC. A zero byte where a chunk id should begin advances by one byte (pad handling).
2. **COMM** gives channels (1–2), bits (16/24) and rate. The rate is the 80-bit float via `getLongDoubleBe` @0x1028115f4, truncated to int.
3. **AIFC compression.** Read only when COMM size > 21:
   - NONE, `twos`, `in24` → big-endian. The constructor sets +0xa8 = 1.
   - `sowt` → little-endian.
   - Anything else → fail.
4. **SSND.** Data starts at chunk + 16; the offset field is ignored. Length = `(ssnd_size − 8) / (bytes·channels)`, or the file remainder if the chunk claims more. A size of 0 fails.
5. `fileRead` @0x10281172c byte-swaps big-endian samples and then converts like WAV.

### JUCE fallback (`JuceAudioReaderWrapper` @0x10283e2c0 / `fileRead` @0x10283e8ec)
- Reads with `AudioFormatReader::read(AudioBuffer<float>*, …, true, true)` into two channels.
- Mono goes to both channels, and integers are scaled as in §3.
- Chunks are word-aligned. The data chunk gives the length.
- Corpus files that take this path: `wav_48k24` and `wav_96k32f`. ffmpeg writes WAVE_FORMAT_EXTENSIBLE for >16-bit and >48 kHz.

## 5. MP3 (`FileReadMp3`)

### Frame scan (`FrameAnalyzer::analyze` @0x102819158)
rekordbox's own scanner builds the frame table. mpg123 never sees the whole file.

**Tables.** Sample rates `M1_SRTbl` {44100, 48000, 32000} @0x1038c5940 and `M2_SRTbl` {22050, 24000, 16000} @0x1038c594c. Bitrates `M1_BRTbl` @0x1038c57c0 (layer I/II/III rows). `M2_BRTbl` @0x1038c5880 holds the MPEG-2 layer III row three times, so MPEG-2 layer I/II use layer III rates.

**The scan, step by step:**
1. **ID3v2.** `"ID3"` with version 2.0, 3.0, 3.1 or 4.0 (computed as `rev + 10·major`) skips `syncsafe size + 10`. A v2.4 footer is not handled. Any other version skips 3 bytes.
2. **Candidate header.** Requires `FF`, byte1 ≥ 0xF0 (MPEG-2.5 is rejected), layer ≠ 0, bitrate index 1..14 and sample-rate index < 3.
3. **Version handling.** Only the MPEG-1 bit is used. MPEG-1 = 1152 samples per frame, otherwise 576, even for layer I/II.
4. **Frame length.** `(long)((spf/8 · kbps·1000) / rate)` in double precision, plus the padding bit.
5. **First frame.** `checkContinuousMpegSync` @0x10281ad64 needs this header and the next two to share a signature: byte 0, byte 1 without the CRC bit, the rate bits and the channel-mode bits. `TAG` or EOF stops the scan.
   - Then `checkXingVbri` @0x10281ab60 runs. `"Xing"` with the frames flag set counts as a VBR header. `"Info"` sets the info flag. `"VBRI"` is checked at +36 after the side info, which is 32 for MPEG-1 stereo.
   - A Xing/Info/VBRI frame **is kept in the table**, and *music start* becomes 1152 (+0x30). The LAME delay and padding fields are never read.
6. **Later frames** must match the first frame's version, layer, rate index and channel mode.
   - With fewer than 4 frames found, a mismatch restarts the scan one byte after the last accepted frame. The music-start offset is kept.
   - Otherwise a mismatching header is skipped byte by byte. The fifth mismatch ends the scan.
7. **Length** = (frames in the table) × spf, including a leading Xing/Info frame.

**Consequence for ffmpeg/libmp3lame files.** They write the Info/Xing frame in plain stereo mode while the audio is joint stereo. The continuity check rejects that frame, so the table starts at the first audio frame. This holds for all corpus MP3s. A LAME CLI file, whose Xing frame matches the stream, keeps it as 1152 samples of silence.

### mpg123 (`decoderOpen` @0x102819b14)
The library is loaded from beside the executable (`Mpg123DynamicLibraryHandle::loadMpg123DynamicLibrary` @0x102818d90), followed by `mpg123_init`. musicbox loads the same `libmpg123.0.dylib` from the installed app. It is universal, current version 38.1.

The setup calls, in order:
1. `mpg123_new(NULL)`
2. `mpg123_param(VERBOSE, 0)`
3. `mpg123_param(FLAGS, 0x300)`. This is SEEKBUFFER | FUZZY, and it **replaces** the default flags, so GAPLESS is off.
4. `mpg123_param(INDEX_SIZE, −1)`
5. `mpg123_format_none`
6. `mpg123_format(rate, channels, MPG123_ENC_SIGNED_16)`
7. `mpg123_open_feed`

`decoderOpen` then feeds audio frames until `mpg123_decode_frame` returns anything other than NEED_MORE. This consumes mpg123's one-time NEW_FORMAT notice. Every later `open_feed` reset decodes straight away. Without this priming the first frame would be lost.

### Decode (`decode` @0x10281a528, `AbstractFileReadDec::fileRead` @0x10280d3d8)
- **Feeding.** One frame per call: `min(size, 0x2000)` bytes from the frame's offset, then one `mpg123_decode_frame`.
- **NEED_MORE.** mpg123 reads ahead to the next header, so the first frame after a reset returns NEED_MORE. rekordbox then adds that frame's span to a lag counter (+0xfc) and the frame index (+0xa8), and feeds the next frame. Output therefore stays aligned: frame *k*'s audio lands at frame *k*'s position.
- **Short output.** Output shorter than a full frame (channels·spf·2 bytes) is treated as empty.
- **Frames before music start** (the Xing frame) produce silence of their span.
- **Seeking.** A read at a position other than the next expected one binary-searches the frame table and restarts with `open_feed` **five frames early** (`max(i,5) − 5`). If that decode fails, one frame of silence is written and the read is retried.
- **No trimming.** Encoder delay and padding are never removed.
- **Timeline.** For LAME/ffmpeg files the timeline is the original + 576 (encoder delay) + 529 (decoder delay) = **+1105 samples**.

## 6. MP4: AAC (`FileReadMp4`) and ALAC (`FileReadAlac`)

### Sample tables
The parsers (`FileReadMp4::getFrameMap` @0x10281cca8 for AAC, `Mp4AtomParser::getEntireMp4FrameMap` @0x10284292c for ALAC) work like this:
1. Flatten the atoms in file order and take the first `stts`, `stsc`, `stco`/`co64` and `stsz`.
2. Place packet *i* at the sum of the `stts` deltas before it. Its frame count is its delta.
3. **Length** is the sum of all deltas. If `stts` runs out before `stsz`, the length stays 0 and the open fails.

`elst`, `iTunSMPB` and the AudioFile packet-table priming/remainder are ignored, so **nothing is trimmed**.

The AAC map shifts deltas left by one when the AudioSpecificConfig signals SBR. ALAC takes the length as `max(parser +0x4c, Σ stts)`; +0x4c is only set for FLAC-in-MP4.

### AAC through AudioToolbox
`fileOpen` @0x1028205a0 opens the file with `AudioFileOpenURL` (read permission, no hint). `analyze` @0x102820700 checks:
- the `esds` and `mdat` atoms;
- 1–2 channels and 16-bit, from the parsed config;
- scale 2⁻¹⁵ (`ldexpf(1,15)`).

`decoderOpen` @0x102820c5c then sets up the converter:
1. Gets source `dfmt`. The format ID must be one of `aac ` / `aach` / `aacl` / `aacp` / `aacs`.
2. If `dfmt.mSampleRate · 2` equals the parsed rate (HE-AAC core rate), it halves the rate, the length and every frame-map entry.
3. Takes the channel count from `dfmt`.
4. Creates `AudioConverterNew(dfmt, lpcm s16)`: flags 0x0C (signed | packed), from the constant @0x1038d8840; 2·ch bytes per frame; at that rate.
5. Copies the magic cookie (`mgic` → `dmgc`).
6. If `pnfo` (packet table info) is readable, **sets `kAudioConverterPrimeInfo` = {0, 0}**.

`decode` @0x102820fe0 works packet by packet:
1. Calls `AudioConverterFillComplexBuffer` asking for the packet's frame count (fewer than 2 → `dfmt.mFramesPerPacket`).
2. The input callback (`converterCallback` @0x102821154) hands out one packet per call. It reads `AudioFileReadPacketData(file, cache = 1, …, index)`. The packet count is capped at 0x800/(2·ch), and the packet index advances by one per callback.
3. A non-zero status or zero frames ends decoding.
4. On a seek, `AudioConverterReset` runs and decoding restarts five packets early.

With priming at zero and no trimming, the output starts with the encoder priming. The corpus AAC files come from Apple's encoder (via ffmpeg `aac_at`), so priming is 2112 and the timeline is the original **+2112 samples**.

**The decode depends on the stack alignment** (verified 2026-10-12, macOS 27.0, M4 Pro):
- **The effect.** Apple's AAC decoder does not produce the same samples for every stack pointer at which `AudioConverterFillComplexBuffer` is entered:
  - Entered with SP ≡ 48 (mod 64), the decode differs from SP ≡ 0, 16 or 32 by ±1 in a few samples, on every corpus file tested.
  - Only the fill calls matter. `AudioConverterNew` and the `dmgc`/`prim` property calls can run at any alignment.
  - A mix of alignments over the calls of one track gives yet other decodes.
  - Only one decoder is installed (`adec`/`aac `/`appl`; `kAudioFormatProperty_Decoders`). The input/output buffer alignment, the buffer contents, the QoS class and the core type (P or E) make no difference.
- **Why it varied between runs.** A caller's stack position depends on the call chain and, on the main thread, on the size of argv and the environment. So the same program gave different decodes in different processes. Concurrent processes with different environments produced both variants, which looked like random switching.
- **rekordbox** makes the call from `FileReadMp4::decode` on its analysis thread. Its call chain is fixed, so its decodes are stable (identical values in both oracle sessions).
- **mb-audio** enters `AudioConverterFillComplexBuffer` through a small trampoline (`mb_audio_call_at`, `coreaudio.rs`), at SP ≡ 0 (mod 1024) whatever the caller's stack.
  - The 0/16/32 class matches rekordbox: with it, auto-gain is bit-exact on 169/169 tracks and all eight waveform tags are byte-exact on 19/19 AAC tracks, against both oracles.
  - With SP ≡ 48: gain 162/169, PWV3/5/6/7 ±1 on 4–10 tracks.
  - `MB_AAC_FILL_SP=<n>` overrides the alignment, for diagnosis only.
- **Tests and tools.**
  - `crates/mb-audio/tests/aac_stack.rs` decodes a probe from eight stack depths and requires one result; without the trampoline it fails.
  - `cargo run --release -p mb-audio --example aac_hash -- [--dump <dir>] <file>…` prints one decode hash per file.

### ALAC
rekordbox decodes ALAC with Apple's open-source `ALACDecoder` (`Init` @0x102806368, `Decode` @0x1028064c8), statically linked:
- **Requirements** (`FileReadAlac::analyze` @0x1028180e0): 16 or 24 bits, ≤2 channels, max frame size 1–65535.
- **Decode.** Each packet is decoded with the `stts` delta as its frame count and placed at its `stts` position.
- **Seek pre-roll.** `getApproachFrameNum` = 10 packets (`AbstractFileReadDec` @0x10280daec).

mb-audio uses AudioToolbox's ALAC decoder instead: packet by packet, at the source bit depth, and the conversion from §3. ALAC is lossless; the probe decodes **sample-identically** to the WAV original.

The packet read must offer more bytes than the packet's PCM size. An incompressible ("escape") ALAC packet is a few bytes larger than its PCM: 16,388 bytes for 4096 stereo 16-bit frames. With a budget of exactly `frames × bytes per frame`, `AudioFileReadPacketData` fails, and every later packet decodes as silence. One real track went silent after 144 s this way. The ALAC feed therefore offers its whole 64 KiB buffer (2026-10-12, found through waveform parity).

Note that ffmpeg's ALAC files carry 647 × 4096 samples in `stts`/`mdhd` for 60 s of audio. rekordbox uses that full length: 2,650,112 samples → PWV3 = 9014.

## 7. FLAC (`FileReadFlac`)
- **Decoder.** Statically linked libFLAC (`analyze` @0x1028144b8), with MD5 checking on.
- **Length** = STREAMINFO total samples. Unknown (0) or >2 channels fails.
- **Scale** `1/(float)ldexp(1, bits−1)`.
- mb-audio uses symphonia's decoder. It left-aligns samples, which gives the same floats (§3). Verified sample-identical for 16-bit/44.1 kHz and 24-bit/48 kHz.

### PVB2
PVB2 is built only for FLAC, by `CBeatAnalyzeThread::createSeekTable` @0x10015b454 → `FlacAnalyzeManager::createSeekTable(400)` @0x101313e1c:
1. Target *i* is `i · ⌊total/400⌋`.
2. libFLAC skips frame by frame. Every target that falls inside a frame gets `{frame first sample, byte offset from the first frame, frame block size}`.
3. Duplicate points are then removed. The probes have 323–400 points.

The PVB2 total equals the decoded length on all 23 FLACs. `mb_audio::flac_seek` builds the whole table from the frame headers (sync code, CRC-8, consecutive frame or sample numbers; no decoding): **byte-exact on 23/23** (`tests/oracle.rs` `pvb2_matches_rekordbox`, 2026-10-12).

## 8. Sample-rate converter

### `SampleRateConverter` @0x10284b490
- **`Init` @0x10284b76c.** Two `SamplingFunctionSRC<Float2>` (+0x28 and +0x70) are initialised for in→out. The length is `calcOutputNum(inputLength)` on the fresh second instance.
- **Blocks.** The block size is 2048 output samples (+0x4c).
- **`fileRead` @0x10284bab8**, per block:
  1. `nIn = calcInputNum(n)`.
  2. Read `nIn` input samples, 512 at a time, as zeros outside `[0, inputLength)`.
  3. `convertSpecifyingOutputNum` (pull).
- **First read.** `Init` zeroes the input and output positions, so the first sequential read at 0 does not re-prime.
- **Random reads.** `prepareSRCProcessing` @0x10284bd70 handles these:
  1. Resets the converter.
  2. When upsampling, rounds the output position up to a multiple of out/gcd.
  3. Computes the input position.
  4. Primes with the input from `⌊(inPos−30)/k⌋·k` (k = in/gcd), discarding the outputs.
  5. Copies any outputs that lie between the requested and the rounded position.

  Negative priming positions call the reader with a negative position (garbage for WAV). mb-audio reads zeros there; this only matters for seeks into the first 30 input samples.

### Filter parameters (`SamplingFunctionSRC<Float2>::init` @0x10288b2c0)
Both rates must be 8 k–200 k and multiples of 50 (`c_src::init_kaiser` @0x10288b570). 11025 Hz fails, so rekordbox cannot analyse it.

| in → out | cutoff | Kaiser β |
|---|---|---|
| 44100 → 48000 | 0.54 | 8 |
| 48000 → 44100 | 0.52 | 8 |
| 44100 → 96000 / 88200 | 0.58 | 10 |
| 48000 → 96000 | 0.45 | 10 |
| 96000 / 88200 → 44100 | 0.55 | 6 |
| 96000 → 48000 | 0.47 | 10 |
| 44100 → 176400 / 192000 | 0.48 | 9 |
| anything else (e.g. 22050 → 44100, 32000 → 48000) | 0.45 | 12 |

### Design (`c_src<Float2>::initIdxAndIrForKaiser` @0x10288d270)
The prototype is computed in double precision:
- **Size.** L = out/50 phases and M = in/50. The prototype has N = 29·L taps, with fc = cutoff / max(L, M).
- **Tap *k*** = sinc × Kaiser.
  - Sinc: `sin((fc·2π)·n) / (n·π)` with n = k − ⌊N/2⌋; 2·fc at n = 0.
  - Kaiser:
    - x = k·(1/(N−1) doubled)
    - t = x − 1
    - arg = √(1 − fma(x, t, −t)) · β/2, where the inner term is an `fnmsub`
    - window = Σⱼ₌₀²¹ `pow(arg, 2j)/(j!)²` over the same sum with β/2. Each term is accumulated with `fmadd`.
- **Factorials.** j! is formed scalar below 9!. Above that it uses eight 2-lane accumulators combined as (a8·(a6·(a4·a2)))·(a9·(a7·(a5·a3))), then scalar multiplication (lane seed {2, 3} @0x103753aa8).
- **Libm calls.** `pow` and `sin` are the system libm, called for every term.
- **Phases.** Phase p starts at input `⌊M·p/L⌋` and holds 29 taps: `(float)(h[L·(t+⌊Mp/L⌋) + L − M·p − 1] · L)`. The normalisation flag is 0.
- **Cache.** Tables are cached per (in/50, out/50).

### Run (`convert_pull` @0x10288b848, `convolute_fine` @0x10288d01c)
- **State.** A 284-entry Float2 history (28 zeros after reset, write index 28), phase p, and base.
- **Output condition.** An output is possible when `idx[p] − base + 29 ≤ write`.
- **Buffer shift.** After writing slot 283, the last 29 entries move to the front and base += 255.
- **Convolution.** `fmla` from both ends inwards (taps 0 & 28, 1 & 27, …), then the middle tap 14.
- **Counting.** `calcInputNum` @0x10288bafc and `calcOutputNum` @0x10288bed4 simulate the same state machine to count samples.

**Delay.** Output *n* is centred 14.5 input samples late. That is +29 samples at 22.05 k → 44.1 k and +7.25 at 96 k → 48 k. The probes show exactly this lag: the 22.05 kHz click lands 29 samples late, in rekordbox's PWV3 column too.

## 9. PVBR (`.DAT`)
`CAnalyzerIF::notify_400pt_wave` @0x101628e64 fills PVBR for everything except aif/aiff/wav/flac/fla, through a separate legacy MPEG parser: `analyzeMpegFrameOffset` @0x101800d0c → `file_analyze` @0x10222a100 → `mp3_analyze` / `aac_analyze` → `vbr_tbl_create` @0x10222c440.

- **Index** (VBR files only; CBR keeps 400 zeros): entry *i* (1-based) = byte offset, relative to the first audio frame, of frame `⌊frames·i/400⌋ − 8`. It is 0 while `frames·i < 3600`. Code: `mp3::pvbr_index`. It matches all 37 MP3 tables byte for byte.
- **Sample count** = frames × 1152 (MPEG-1) or × 576. For the corpus this equals the decoded length. AAC gets 0.
- **`InMpegFrame`.** `getMpegFrameOffset` @0x101800d78 quantises ms to 1/75 s (`(long)(ms·0.075) · 13.333…`) before looking up byte offsets with the same table. This is used for the cue `InMpegFrame`/`InMpegAbs` columns; open.

## 10. Timeline helpers (`mb_audio::timeline`)
- Analysis rate 44.1/48 kHz.
- A 1/150 s column = 294 / 320 samples.
- Columns = ⌈(L−1)/spc⌉.
- ms = sample·1000/rate.
- `sample_to_ms_us` splits a position into whole ms and µs, like PCP2/PQT2 store it. How rekordbox rounds cue and grid positions to ms is an M5 question; see the open questions.

## 11. Verification (2026-10-12, rekordbox 7.2.19, macOS 27.0)

**Probe `click_128_offset1234`** (first beat 1.234 s), decoded with mb-audio and compared with the WAV original at the same output rate (cross-correlation around the onset):

| Format | rekordbox reader | Rate (file → decoded) | Decoded length | Lag vs WAV (samples) | Residual | rekordbox PWV3 / ours | First loud column (rb / ours) | PQTZ beat 0 |
|---|---|---|---|---|---|---|---|---|
| WAV 44.1k/16 | FileReadWav | 44100 | 2,646,000 | 0 | 0 | 9000 / 9000 | 185 / 185 | 1231 |
| AIFF 44.1k/16 | FileReadAiff | 44100 | 2,646,000 | 0 | 0 | 9000 / 9000 | 185 / 185 | 1231 |
| FLAC 44.1k/16 | FileReadFlac | 44100 | 2,646,000 | 0 | 0 | 9000 / 9000 | 185 / 185 | 1231 |
| FLAC 48k/24 | FileReadFlac | 48000 | 2,880,000 | 0 | 0 | 9000 / 9000 | 185 / 185 | 1231 |
| WAV 48k/24 (extensible) | JUCE | 48000 | 2,880,000 | 0 | 0 | 9000 / 9000 | 185 / 185 | 1231 |
| ALAC 44.1k/16 | FileReadAlac | 44100 | 2,650,112 | 0 | 0 | 9014 / 9014 | 185 / 185 | 1231 |
| AAC 256k | FileReadMp4 | 44100 | 2,648,128 | **+2112** | 1.2e-3 | 9008 / 9008 | 192 / 192 | 343 |
| MP3 CBR 320 (Info frame) | FileReadMp3 | 44100 | 2,647,296 | **+1105** | 4.3e-4 | 9005 / 9005 | 188 / 188 | 320 |
| MP3 VBR V0 (Xing) | FileReadMp3 | 44100 | 2,647,296 | **+1105** | 2.1e-4 | 9005 / 9005 | 188 / 188 | 320 |
| MP3 CBR 128, no Xing | FileReadMp3 | 44100 | 2,647,296 | **+1105** | 4.1e-2 | 9005 / 9005 | 188 / 188 | 320 |
| WAV 22.05k/16 mono | FileReadWav + SRC | 22050 → 44100 | 2,646,000 | **+29** | 3.5e-3 | 9000 / 9000 | 185 / 185 | 1232 |
| WAV 96k/32f (extensible) | JUCE + SRC | 96000 → 48000 | 2,880,000 | **+7** | 4.7e-3 | 9000 / 9000 | 185 / 185 | 296 |

The residual is the maximum absolute difference after alignment; for the lossy rows it is coding error. The PQTZ beat-0 column is rekordbox's own grid start. A grid can start up to two beats (937.5 ms) before the first click and moves by about 1–2 ms with lossy coding, so it is only a coarse cross-check. Two examples: 343 = 1280.5 − 937.5 is consistent with +2112 samples (+47.9 ms); 320 = 1257.5 − 937.5 is consistent with +1105 (+25.1 ms).

**Oracle-a (169 tracks: 100 real, 55 probes, 14 demo/sampler):**

| Check | Result |
|---|---|
| Decoded length → ⌈(L−1)/spc⌉ equals PWV3 | 169/169 |
| MP3 PVBR sample count equals the decoded length | 37/37 |
| FLAC PVB2 total equals the decoded length | 23/23 |
| `mp3::pvbr_index` equals the PVBR index | 37/37 (14 VBR with data, 23 CBR all-zero) |
| End-to-end decode | 169/169 |

Lengths per reader: 37 MP3, 19 AAC, 14 ALAC, 23 FLAC, 34 WAV, 34 AIFF, 8 JUCE. The full decode runs at about 1900× realtime.

**Random access** (as the player uses it) equals the sequential decode exactly for PCM, FLAC, ALAC and both SRC cases. For MP3 and AAC it can differ in the last bit for a few samples after a restart; mpg123 and Apple's decoder keep state across a reset. rekordbox's analysis is strictly sequential from position 0, and its seeks behave the same way. Tests: `crates/mb-audio/tests/{oracle,corpus,synthetic}.rs`.

## 12. Exact vs approximate
**Exact by construction and verified:**
- PCM, FLAC and ALAC samples.
- All lengths.
- The MP3 frame table.
- The PVBR index.
- Format selection and the analysis rate.

**Exact by construction, not observable directly:**
- MP3 samples: same libmpg123, same parameters, same feed/decode sequence including the NEW_FORMAT priming.
- AAC samples: same AudioToolbox calls, same per-packet callback, same prime info, and `AudioConverterFillComplexBuffer` entered at the stack alignment class rekordbox's decodes match (§ 6). Every analysis output that depends on them is exact on all 19 AAC tracks (auto-gain, all eight waveform tags). This depends on the macOS version, which is recorded in the fixture manifest.
- SRC samples: literal port with matching FMA, libm and evaluation order; the delay is confirmed.

**Approximate or untested:**
- HE-AAC / SBR (the halving logic is ported but there is no fixture).
- MPEG-2/2.5 and layer I/II MP3.
- JUCE AIFF variants (8/32-bit, `fl32`).
- `.aac` (ADTS) and Ogg are not implemented.
- Player rate conversion is linear interpolation (preview only).

## 13. Open questions
- **MP3 timeline:** a real LAME-CLI MP3 with a matching Xing frame should play 1152 samples of silence first (+2257 total). Confirm with a probe encoded by `lame`, analysed by rekordbox.
- **Cue/grid rounding:** how `djplay` rounds sample positions to the ms (PCOB/PCO2, `djmdCue.InMsec`) and to µs (PCP2 +0x1e, PQT2 entries). Needed for M5 cue editing.
- **Ogg and `.aac`:** whether `.ogg` really goes through JUCE's `OggVorbisAudioFormat`, and the `FileReadAac` (ADTS) frame map.
- **SRC confirmation:** run M6 waveform/beat parity on the 22.05 kHz and 96 kHz probes as the end-to-end check of the port.
- **`InMpegFrame`/`InMpegAbs` derivation:** `GetMpegFrmOfst` @0x101800a60, `vbr_frm_search` @0x10222c5f8.
