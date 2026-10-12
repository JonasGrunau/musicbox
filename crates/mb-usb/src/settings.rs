//! Player settings files rekordbox puts on a stick: `MYSETTING.DAT`, `MYSETTING2.DAT`,
//! `DJMMYSETTING.DAT`, `DEVSETTING.DAT`, and the KUVO profile `djprofile.nxs`.
//! Spec: `docs/re/export.md` § 5.
//!
//! When rekordbox creates or updates a device database it applies the local My Settings
//! (`MySettingFile::exportToDeviceFromLocalFile` @0x10024f68c, `exportDJMMySettingFileToDevice`
//! @0x1023d68e0), writes `DEVSETTING.DAT` only when the device has no valid one
//! (`updateDeviceDevSettingData` @0x1023ec084), and copies the local `djprofile.nxs` when an
//! export thread starts. The local files live in the settings folder
//! (`~/Library/Application Support/Pioneer/rekordbox6/`); a missing local file means the
//! defaults below. A device file that already has the bytes rekordbox would write is left alone.

use std::path::Path;

/// The four DAT files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsKind {
    MySetting,
    MySetting2,
    DjmMySetting,
    DevSetting,
}

impl SettingsKind {
    pub const ALL: [SettingsKind; 4] = [
        SettingsKind::MySetting,
        SettingsKind::MySetting2,
        SettingsKind::DjmMySetting,
        SettingsKind::DevSetting,
    ];

    pub fn file_name(self) -> &'static str {
        match self {
            SettingsKind::MySetting => "MYSETTING.DAT",
            SettingsKind::MySetting2 => "MYSETTING2.DAT",
            SettingsKind::DjmMySetting => "DJMMYSETTING.DAT",
            SettingsKind::DevSetting => "DEVSETTING.DAT",
        }
    }

    /// Data block length N (header field 0x64).
    pub fn data_len(self) -> usize {
        match self {
            SettingsKind::MySetting | SettingsKind::MySetting2 => 0x28,
            SettingsKind::DjmMySetting => 0x34,
            SettingsKind::DevSetting => 0x20,
        }
    }

    /// (brand, product, version) rekordbox writes.
    fn header(self) -> (&'static str, &'static str, &'static str) {
        match self {
            SettingsKind::MySetting | SettingsKind::MySetting2 => ("PIONEER", "rekordbox", "0.001"),
            SettingsKind::DjmMySetting => ("PioneerDJ", "rekordbox", "1.000"),
            // `getFileVersion(true)`: the app version cut to three parts.
            SettingsKind::DevSetting => ("PIONEER DJ", "rekordbox", "7.2.19"),
        }
    }

    /// Offset and length (within the data block) of the setting bytes whose bit 7 means "set"
    /// and which the merge rule applies to.
    fn setting_bytes(self) -> std::ops::Range<usize> {
        match self {
            SettingsKind::MySetting => 8..40,
            SettingsKind::MySetting2 => 0..0x28,
            SettingsKind::DjmMySetting => 12..44,
            SettingsKind::DevSetting => 8..0x20,
        }
    }

    /// The data block of a fresh library (`setDefaultContent`; DJM: `DJMMSData::setDefault`
    /// @0x1007d4cf8 with `generalSettingDefine` @0x1007d3ce4 defaults, each `| 0x80`).
    pub fn default_data(self) -> Vec<u8> {
        let mut d = vec![0u8; self.data_len()];
        match self {
            SettingsKind::MySetting => {
                d[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
                d[4..8].copy_from_slice(&2u32.to_le_bytes());
                d[8..40].copy_from_slice(&[
                    0x81, 0x83, 0x81, 0x88, 0x81, 0x01, 0x82, 0x81, 0x81, 0x01, 0x01, 0x01, 0x82,
                    0x80, 0x80, 0x81, 0x80, 0x81, 0x80, 0x00, 0x00, 0x81, 0x00, 0x00, 0x81, 0x81,
                    0x81, 0x80, 0x81, 0x80, 0x00, 0x00,
                ]);
            }
            SettingsKind::MySetting2 => {
                d[..13].copy_from_slice(&[
                    0x81, 0x80, 0x83, 0x83, 0x81, 0x80, 0x80, 0x00, 0x00, 0x00, 0x80, 0x81, 0x85,
                ]);
            }
            SettingsKind::DjmMySetting => {
                d[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
                d[4..8].copy_from_slice(&1u32.to_le_bytes());
                d[8..12].copy_from_slice(&0x20u32.to_le_bytes());
                const DEFAULTS: [u8; 18] = [1, 2, 0, 0, 1, 1, 0, 1, 0, 0, 5, 2, 0, 0, 2, 4, 1, 1];
                for (i, v) in DEFAULTS.iter().enumerate() {
                    d[12 + i] = v | 0x80;
                }
                // 44..52: the empty BFX block (count 0, size 0).
            }
            SettingsKind::DevSetting => {
                d[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
                d[4..8].copy_from_slice(&1u32.to_le_bytes());
                d[8..17].fill(1);
            }
        }
        d
    }
}

/// CRC-16/XMODEM (poly 0x1021, init 0), as `pioneerMySetting::calcCRC` @0x10024dd34.
pub fn crc16_xmodem(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in bytes {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A parsed settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsFile {
    pub brand: String,
    pub product: String,
    pub version: String,
    pub data: Vec<u8>,
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn put_cstr(out: &mut [u8], s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(out.len() - 1);
    out[..n].copy_from_slice(&b[..n]);
}

impl SettingsFile {
    /// Parses and validates a file of `kind` (string block length, data length, CRC).
    pub fn parse(kind: SettingsKind, bytes: &[u8]) -> Option<Self> {
        let n = kind.data_len();
        if bytes.len() != 0x68 + n + 4 {
            return None;
        }
        if u32::from_le_bytes(bytes[0..4].try_into().ok()?) != 0x60
            || u32::from_le_bytes(bytes[0x64..0x68].try_into().ok()?) as usize != n
        {
            return None;
        }
        let stored = u16::from_le_bytes([bytes[0x68 + n], bytes[0x69 + n]]);
        if stored != crc(kind, &bytes[..0x68 + n]) {
            return None;
        }
        Some(Self {
            brand: cstr(&bytes[0x04..0x24]),
            product: cstr(&bytes[0x24..0x44]),
            version: cstr(&bytes[0x44..0x64]),
            data: bytes[0x68..0x68 + n].to_vec(),
        })
    }

    /// The file bytes with a fresh CRC.
    pub fn to_bytes(&self, kind: SettingsKind) -> Vec<u8> {
        let n = kind.data_len();
        let mut out = vec![0u8; 0x68 + n + 4];
        out[0..4].copy_from_slice(&0x60u32.to_le_bytes());
        put_cstr(&mut out[0x04..0x24], &self.brand);
        put_cstr(&mut out[0x24..0x44], &self.product);
        put_cstr(&mut out[0x44..0x64], &self.version);
        out[0x64..0x68].copy_from_slice(&(n as u32).to_le_bytes());
        let m = self.data.len().min(n);
        out[0x68..0x68 + m].copy_from_slice(&self.data[..m]);
        let c = crc(kind, &out[..0x68 + n]);
        out[0x68 + n..0x6a + n].copy_from_slice(&c.to_le_bytes());
        out
    }
}

/// MYSETTING, MYSETTING2 and DEVSETTING checksum the data block; DJMMYSETTING the whole file up
/// to the CRC (`DJMMSContent::calcCRC` @0x1007d6004).
fn crc(kind: SettingsKind, upto_crc: &[u8]) -> u16 {
    match kind {
        SettingsKind::DjmMySetting => crc16_xmodem(upto_crc),
        _ => crc16_xmodem(&upto_crc[0x68..]),
    }
}

/// `saveDeviceFile` @0x10024fec4: every setting byte whose bit 7 is set in `source` replaces
/// the low 7 bits of the device byte; the device keeps its own bit 7.
pub fn merge_settings(kind: SettingsKind, device: &[u8], source: &[u8]) -> Vec<u8> {
    let mut out = device.to_vec();
    for i in kind.setting_bytes() {
        if let (Some(s), Some(d)) = (source.get(i), out.get_mut(i))
            && s & 0x80 != 0
        {
            *d = (*d & 0x80) | (s & 0x7f);
        }
    }
    out
}

/// The bytes rekordbox would leave in `<PIONEER>/<file>` for `kind`, given the local settings
/// file (`None` = missing → defaults) and the current device file. `None` = leave the device
/// file as it is.
pub fn device_settings_bytes(
    kind: SettingsKind,
    local: Option<&[u8]>,
    device: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let source = local
        .and_then(|b| SettingsFile::parse(kind, b))
        .map(|f| f.data)
        .unwrap_or_else(|| kind.default_data());
    let current = device.and_then(|b| SettingsFile::parse(kind, b));
    let (brand, product, version) = kind.header();
    let file = match (kind, current) {
        // DEVSETTING is only written when the device has no valid one.
        (SettingsKind::DevSetting, Some(_)) => return None,
        // MYSETTING2 keeps the header a player wrote.
        (SettingsKind::MySetting2, Some(cur)) => SettingsFile {
            data: merge_settings(kind, &cur.data, &source),
            ..cur
        },
        (_, Some(cur)) => SettingsFile {
            brand: brand.into(),
            product: product.into(),
            version: version.into(),
            data: merge_settings(kind, &cur.data, &source),
        },
        (_, None) => SettingsFile {
            brand: brand.into(),
            product: product.into(),
            version: version.into(),
            data: source,
        },
    };
    let bytes = file.to_bytes(kind);
    (device != Some(bytes.as_slice())).then_some(bytes)
}

/// Every settings file to write into `pioneer_dir`, reading the local files from
/// `settings_dir` (`None` = no local files: defaults). `apply_my_settings` is rekordbox's
/// preference "apply My Settings to devices"; DEVSETTING is written regardless.
pub fn settings_writes(
    settings_dir: Option<&Path>,
    pioneer_dir: &Path,
    apply_my_settings: bool,
) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for kind in SettingsKind::ALL {
        if kind != SettingsKind::DevSetting && !apply_my_settings {
            continue;
        }
        let local = settings_dir.and_then(|d| std::fs::read(d.join(kind.file_name())).ok());
        let target = pioneer_dir.join(kind.file_name());
        let device = std::fs::read(&target).ok();
        if let Some(bytes) = device_settings_bytes(kind, local.as_deref(), device.as_deref()) {
            out.push((target, bytes));
        }
    }
    if let Some(profile) = settings_dir
        .map(|d| d.join("djprofile.nxs"))
        .and_then(|p| std::fs::read(p).ok())
    {
        let target = pioneer_dir.join("djprofile.nxs");
        if std::fs::read(&target).ok().as_deref() != Some(profile.as_slice()) {
            out.push((target, profile));
        }
    }
    out
}

/// One preference from rekordbox's `rekordbox3.settings` in `settings_dir` (`<VALUE name="…"
/// val="…"/>` entries). `None` when the file or the entry is missing.
pub fn preference(settings_dir: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(settings_dir.join("rekordbox3.settings")).ok()?;
    let key = format!("name=\"{name}\"");
    let at = text.find(&key)? + key.len();
    let rest = &text[at..text[at..].find('>').map_or(text.len(), |e| at + e)];
    let start = rest.find("val=\"")? + 5;
    Some(rest[start..start + rest[start..].find('"')?].to_owned())
}

/// The preference `HotCueColorType` (default colours of hot cues without a colour code,
/// `crate::cues::default_hot_cue_code`).
pub fn hot_cue_color_type(settings_dir: &Path) -> Option<u8> {
    preference(settings_dir, "HotCueColorType")?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_xmodem() {
        assert_eq!(crc16_xmodem(b"123456789"), 0x31c3);
    }

    #[test]
    fn defaults_match_a_fresh_stick() {
        // CRCs of rekordbox's files on the oracle stick (fixtures/oracle-a/rbtest/PIONEER).
        let expect = [
            (SettingsKind::MySetting, 148, 0x7a48),
            (SettingsKind::MySetting2, 148, 0x9c7a),
            (SettingsKind::DjmMySetting, 160, 0x07b0),
            (SettingsKind::DevSetting, 140, 0x98e1),
        ];
        for (kind, len, crc) in expect {
            let bytes = device_settings_bytes(kind, None, None).unwrap();
            assert_eq!(bytes.len(), len, "{kind:?}");
            let n = kind.data_len();
            assert_eq!(
                u16::from_le_bytes([bytes[0x68 + n], bytes[0x69 + n]]),
                crc,
                "{kind:?}"
            );
            let parsed = SettingsFile::parse(kind, &bytes).unwrap();
            assert_eq!(parsed.data, kind.default_data());
            // Writing again onto the same bytes changes nothing.
            assert_eq!(device_settings_bytes(kind, None, Some(&bytes)), None);
        }
    }

    #[test]
    fn merge_keeps_device_bit7_and_header() {
        let kind = SettingsKind::MySetting2;
        let mut player = SettingsFile {
            brand: "PIONEER DJ".into(),
            product: "CDJ-3000".into(),
            version: "0.001".into(),
            data: vec![0u8; kind.data_len()],
        };
        player.data[0] = 0x85;
        let mut local = SettingsFile {
            brand: "PIONEER".into(),
            product: "rekordbox".into(),
            version: "0.001".into(),
            data: vec![0u8; kind.data_len()],
        };
        local.data[0] = 0x82;
        local.data[1] = 0x03; // bit 7 clear: not applied
        let out = device_settings_bytes(
            kind,
            Some(&local.to_bytes(kind)),
            Some(&player.to_bytes(kind)),
        )
        .unwrap();
        let parsed = SettingsFile::parse(kind, &out).unwrap();
        assert_eq!(parsed.product, "CDJ-3000");
        assert_eq!(parsed.data[0], 0x82);
        assert_eq!(parsed.data[1], 0x00);
    }

    #[test]
    fn devsetting_is_never_replaced() {
        let kind = SettingsKind::DevSetting;
        let mut other =
            SettingsFile::parse(kind, &device_settings_bytes(kind, None, None).unwrap()).unwrap();
        other.data[8] = 2;
        assert_eq!(
            device_settings_bytes(kind, None, Some(&other.to_bytes(kind))),
            None
        );
    }
}
