# My Settings field map (MYSETTING, MYSETTING2, DJMMYSETTING, DEVSETTING)

Status: **verified** from the binary (getters, setters, range checks and the editor's menu code, addresses below). The map is consistent with every byte of the reference stick (written partly by a CDJ-3000), the oracle stick and the local files (rekordbox 7.2.19 defaults). That a player reads each value the way rekordbox's menus label it is **not tested on hardware**: see the CDJ check in `docs/progress.md`.

File layout, header strings, CRC and when rekordbox writes these files: `export.md` § 5. Byte numbers below are offsets in the **data block** (file offset = 0x68 + byte). Code: `crates/mb-usb/src/mysettings.rs` (`FIELDS` is this table), UI `apps/musicbox/src/mysettings_ui.rs`.

## Encoding
- **MYSETTING, MYSETTING2, DJMMYSETTING:** each setting is one byte, `0x80 | value`. Without bit 7 the setting is *unset*: rekordbox's getter then returns a fixed value (the "unset" column below, equal to the factory default), and the export merge skips it (`export.md` § 5). Players leave fields unset too: the reference stick's CDJ-3000-written MYSETTING2 has jog display mode, pad brightness, gate cue and overwrite-cue unset.
- **DEVSETTING:** plain values; a value outside the valid set reads as 1.
- **Menu order ≠ value order** for several fields. The `(value, label)` lists below are in rekordbox's menu order (`MySettingsEditor::getItemComboBoxItems` @0x101af2c58; `updateItemComboBoxes` @0x101af33a0 maps value → menu index).

## CDJ/XDJ: `pioneerMySetting::MySettingData`
Getters/setters @0x10024e72c–0x10024f01c. The object holds MYSETTING at +0x08 (data at +0x70) and MYSETTING2 at +0x9c (data at +0x104). Range repair on load: `checkDataChunkInvalidField` @0x10024e234 (MYSETTING, per field, to the value in brackets); MYSETTING2 is validated as a whole by `isValidExtDataChunk` @0x10024dee4 and ignored if any field is out of range.

| File | Byte | Label (rekordbox) | Values in menu order | Unset |
|---|---|---|---|---|
| MYSETTING | 8 | ON AIR DISPLAY | 0 OFF, 1 ON | 1 |
| MYSETTING | 9 | LCD BRIGHTNESS (Main Display) | 1–5 (menu by ID) | 3 |
| MYSETTING | 10 | QUANTIZE | 0 OFF, 1 ON | 1 |
| MYSETTING | 11 | AUTO CUE LEVEL | 7 −78dB, 6 −72dB, 5 −66dB, 4 −60dB, 3 −54dB, 2 −48dB, 1 −42dB, 0 −36dB, 8 MEMORY CUE, 9 HOT CUE (`menu = 7 − value` for value < 8, table @0x1037f1968) | 8 |
| MYSETTING | 12 | LANGUAGE | 1 English, 2 French, 3 German, 4 Italian, 5 Dutch, 6 Spanish, 7 Russian, 8 Korean, 9 Chinese simplified, 10 Chinese traditional, 11 Japanese, 12 Portugal, 13 Swedish, 14 Czech, 15 Hungarian, 16 Danish, 17 Greek, 18 Turkish (menu by ID; valid 1–18) | 1 |
| MYSETTING | 14 | JOG RING BRIGHTNESS | 0 OFF, 1 "1 (Dark)", 2 "2 (Bright)" | 2 |
| MYSETTING | 15 | JOG RING INDICATOR | 0 OFF, 1 ON | 1 |
| MYSETTING | 16 | SLIP FLASHING | 0 OFF, 1 ON | 1 |
| MYSETTING | 20 | DISC SLOT ILLUMINATION | 0 OFF, 1 "1 (Dark)", 2 "2 (Bright)" | 2 |
| MYSETTING | 21 | EJECT/LOAD LOCK | 0 UNLOCK, 1 LOCK | 0 |
| MYSETTING | 22 | SYNC (BPM/BEAT) | 0 OFF, 1 ON | 0 |
| MYSETTING | 23 | PLAY MODE / AUTO PLAY MODE | 0 CONTINUE / ON, 1 SINGLE / OFF | 1 |
| MYSETTING | 24 | QUANTIZE BEAT VALUE | 3 1/8 Beat, 2 1/4 Beat, 1 1/2 Beat, 0 1 Beat (table @0x1037f1988) | 0 |
| MYSETTING | 25 | HOT CUE AUTO LOAD | 0 OFF, 2 rekordbox SETTING, 1 ON | 1 |
| MYSETTING | 26 | HOT CUE COLOR | 0 OFF, 1 ON | 0 |
| MYSETTING | 29 | NEEDLE LOCK | 0 UNLOCK, 1 LOCK | 1 |
| MYSETTING | 32 | TIME MODE | 0 Elapsed, 1 REMAIN | 1 |
| MYSETTING | 33 | JOG MODE | 1 VINYL, 0 CDJ | 1 |
| MYSETTING | 34 | AUTO CUE | 0 OFF, 1 ON | 1 |
| MYSETTING | 35 | MASTER TEMPO | 0 OFF, 1 ON | 0 |
| MYSETTING | 36 | TEMPO RANGE | 0 +/-6, 1 +/-10, 2 +/-16, 3 WIDE | 1 |
| MYSETTING | 37 | PHASE METER | 0 TYPE 1, 1 TYPE 2 | 0 |
| MYSETTING2 | 0 | VINYL SPEED ADJUST | 0 TOUCH & RELEASE, 1 TOUCH, 2 RELEASE | 1 |
| MYSETTING2 | 1 | JOG DISPLAY MODE | 0 AUTO, 1 INFO, 2 SIMPLE, 3 ARTWORK | 0 |
| MYSETTING2 | 2 | PAD/BUTTON BRIGHTNESS | 1–4 (menu by ID) | 3 |
| MYSETTING2 | 3 | JOG LCD BRIGHTNESS (Deck Display) | 1–5 (menu by ID) | 3 |
| MYSETTING2 | 4 | WAVEFORM DIVISIONS | 0 TIME SCALE, 1 PHRASE | 1 |
| MYSETTING2 | 5 | GATE CUE | 0 OFF, 1 ON | 0 |
| MYSETTING2 | 6 | OVERWRITE CURRENT CUE WITH HOT CUE (SMART CUE) | 0 OFF, 1 ON | 0 |
| MYSETTING2 | 10 | WAVEFORM / PHASE METER | 0 WAVEFORM, 1 PHASE METER | 0 |
| MYSETTING2 | 12 | BEAT JUMP BEAT VALUE | 0 1/2 Beat, 1 1 Beat, 2 2 Beat, 3 4 Beat, 4 8 Beat, 5 16 Beat, 6 32 Beat, 7 64 Beat | 5 |

**Not named** (rekordbox neither reads nor shows them; kept as they are): MYSETTING bytes 13, 17–19 (default `01`, i.e. unset), 27, 28, 30, 31, 38, 39 (default 0); MYSETTING2 bytes 7–9, 11 (default `0x81`, validated 0–2) and 13–39.

## DJM: `pioneerMySetting::DJMMSData` / `DJMMSContent`
32 setting bytes at data 12–43 (setting `i` = byte 12 + i). `generalSettingDefine` @0x1007d3ce4 gives (min, max, default, optional) for i < 18; `DJMMSContent::setValue` @0x1007d5bc4 writes `0x80 | value` only inside (min, max); `setDisable` @0x1007d5b60 writes 0 (the whole byte); `DJMMSData::getValue` @0x1007d50b8 returns the value when bit 7 is set and it is in range, else the default. Editor: `MySettingsEditor::onDJMMySettingChanged` @0x101af5d64.

| Setting | Label | Values in menu order | Default |
|---|---|---|---|
| 0 | CH FADER CURVE | 0, 1, 2 | 1 |
| 12 | CH FADER CURVE (LONG FADER) | 0, 1, 2 | 0 |
| 1 | CROSSFADER CURVE | 0, 1, 2 | 2 |
| 2 | PRE EQ (A) (headphones) | 0 POST EQ, 1 PRE EQ | 0 |
| 3 / 13 | MONO SPLIT (A) | STEREO, MONO SPLIT, NONE (see below) | 0 / 0 |
| 14 | MONO SPLIT (B) | 0 STEREO, 1 MONO SPLIT, 2 NONE | 2 |
| 4 | BEAT FX QUANTIZE | 0 OFF, 1 ON | 1 |
| 5 | MIC LOW CUT | 0 OFF, 1 ON(for MC) | 1 |
| 16 / 17 | BEAT (mic echo) | 1/8, 1/4, 1/2, 3/4, 1, 2, 4, 8 as numerator / denominator (1,8) (1,4) (1,2) (3,4) (1,1) (2,1) (4,1) (8,1); unknown pairs read as "1" | 1 / 1 |
| 6 | MODE (talk over) | 0 ADVANCED, 1 NORMAL | 0 |
| 7 / 15 | LEVEL (talk over) | −∞, −24dB, −18dB, −12dB, −6dB (see below) | 1 / 4 |
| 8 | CH (MIDI) | 0–15 shown as 1–16 | 0 |
| 9 | BUTTON TYPE (MIDI) | 0 TOGGLE, 1 TRIGGER | 0 |
| 10 | DISPLAY (brightness) | 0 WHITE, 1–5 | 5 |
| 11 | INDICATOR (brightness) | 0–2 shown as 1–3 | 2 |

**Paired settings** (the "optional" flag of `generalSettingDefine` is set on 3, 7, 13, 15; exactly one of each pair is enabled after an edit):
- **MONO SPLIT (A):** STEREO / MONO SPLIT write setting 3 = 0 / 1 and disable 13; NONE writes 13 = 2 and disables 3. Reading: setting 13's value when it is enabled, else setting 3's. (*Hypothesis:* the value written to 13 is 2; the decompiler lost the argument, 2 is the only value consistent with the read side.) A model check (`bVar2`) in front of this read is not decoded.
- **Talk over LEVEL:** −∞ disables 7 and writes 15 = 4 (its only valid value); −24…−6 dB write 7 = 0…3 and disable 15. Reading: 7 enabled → menu 1 + value; else 15 enabled → −∞; else −18dB. The level labels are inline short strings in `__GLOBAL__sub_I_MySettingsEditor.cpp` @0x101af7024.
- **Mic echo BEAT:** 16 and 17 are always written together.

**Fixtures:** the oracle stick and the local file hold all 18 defaults. The reference stick's file (older rekordbox) has settings 14–17 at 0 (unset), which read as their defaults.

## DJ System: `cdjDeviceSetting::DeviceSettingData`
Plain bytes; data 0–7 are the magic and version (`export.md` § 5). Setters @0x100c06638–0x100c067e4; getters return 1 for values outside the valid set; `setContent` @0x100c06368 rejects a file with any field outside its set. Labels and values come from the Preferences pages (`DJSystemWaveColor`, `DJSystemWavePosition`, `DJSystemOverviewWave`, `DJSystemKeyOption`, `DJSystemWaveformDivisions`, `DJSystemImageOnJogDisplay`, `DJSystemDisplayUsernameOnScreen`, each `buttonClicked`).

| Byte | Label | Values | Valid set |
|---|---|---|---|
| 9 | Type of the Overview Waveform | 1 Half Waveform, 2 Full Waveform | 0–2 |
| 10 | Waveform color | 1 BLUE, 3 RGB, 4 3Band | 0, 1, 3, 4 |
| 12 | Key display format | 1 Classic, 2 Alphanumeric (`isKeyCategoryDispStyleNornal` = value ≠ 2) | 0–2 |
| 13 | Waveform Current Position | 1 CENTER, 2 LEFT | 0–2 |
| 14 | Waveform Divisions | 1 Time Scale, 2 Phrase | 0–2 |
| 15 | Image On-Jog Display | 1 Artwork, 2 Custom Image | 0–2 |
| 16 | Display username on-screen | 1 OFF, 2 ON | 0–2 |

- Bytes 8 and 11 are not named (byte 11 is validated 0–2); value 0 of the named fields has no menu entry. The CDJ-3000-written file on the reference stick has 0 in bytes 14–16.
- *Hypothesis:* the button order in the pages is the value order (CENTER = 1, LEFT = 2; Half = 1, Full = 2; Time Scale = 1, Phrase = 2). The value/label pairs for colour (1, 3, 4), key display (1 normal = Classic), image (1 Artwork, 2 Custom) and username (toggle on = 2) are read directly from the handlers.

## What musicbox does with it
- `MySettings::load` reads the four files from `<root>/PIONEER` (or `.PIONEER`); a missing file reads as rekordbox's defaults, a file that does not parse (bad size or CRC, e.g. a DJMMYSETTING with a Beat FX block) is shown read-only and never written.
- An edit changes only the setting's own bytes; headers (e.g. `PIONEER DJ` / `CDJ-3000` on player-written files) and unnamed bytes stay; the CRC is recomputed. A missing file is created with rekordbox's header and defaults.
- Saving re-checks that each file is unchanged since it was read, refuses the reference stick (UUID) and protected volumes, and refuses while rekordbox runs. Files are written next to their name, `F_FULLFSYNC`ed and renamed into place.
- The local copies in rekordbox's settings folder are not edited (rekordbox's own My Settings page does that); an export still applies them (`export.md` § 5).

## Open questions
- [ ] Hardware: does a CDJ-3000 / DJM-900NXS2 apply each value as labelled (the CDJ check)?
- [ ] The unnamed bytes listed above.
- [ ] The MONO SPLIT (A) model check and the exact value written to setting 13.
- [ ] Settings 18–31 of DJMMYSETTING and the Beat FX block (0x480-byte files).
