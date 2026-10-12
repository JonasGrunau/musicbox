//! Device → local analysis files, as rekordbox's import from a device does it
//! (`docs/re/ops/import-device.md` § Analysis files).
//!
//! `importFromDeviceThread::import_device_analysis` @0x1001db460 copies the device `.DAT`, `.EXT`
//! and `.2EX` byte for byte next to the new track's local analysis path, then
//! `CAnalyzerIF::initPathAndDeviceCue` @0x10162ee04 opens the `.DAT` and `.EXT`, sets their PPTH
//! to the local form, clears every cue list (`clear_pmai_all_cue_data`,
//! `clear_pmai_ext_all_cue_data`; the collection's cues live in `djmdCue`) and writes both files
//! again (`write_pmai`, `write_pmai_ext`). The `.2EX` is never rewritten: it keeps the device
//! PPTH and its stored (scrambled) PVDI. Neither the EXT reader nor writer calls a scramble
//! function, so PSSI stays as stored on the device too (*hypothesis* from the call graph; both
//! forms are self-describing, `anlz.md` § PSSI/PVDI scramble).

use crate::tags::{CueList, CueListExt, Path};
use crate::{AnlzFile, Section};

/// The local `.DAT` or `.EXT` made from a device file: PPTH = `?/<audio file name>`
/// ([`crate::usb::local_ppth`]), every PCOB/PCO2 list emptied (same list kind), every other
/// section unchanged. Use it for `.DAT` and `.EXT` only; the `.2EX` is copied unchanged.
pub fn to_local(device: &AnlzFile, audio_file_name: &str) -> AnlzFile {
    let ppth = crate::usb::local_ppth(audio_file_name);
    let sections = device.sections.iter().map(|s| local_section(s, &ppth)).collect();
    AnlzFile { header: device.header.clone(), sections }
}

fn local_section(s: &Section, ppth: &str) -> Section {
    match &s.tag.0 {
        b"PPTH" => match Path::decode(s) {
            Ok(mut p) => {
                p.path = ppth.to_owned();
                p.encode()
            }
            Err(_) => s.clone(),
        },
        b"PCOB" => match CueList::decode(s) {
            Ok(l) => CueList::empty(l.kind).encode(),
            Err(_) => s.clone(),
        },
        b"PCO2" => match CueListExt::decode(s) {
            Ok(l) => CueListExt::empty(l.kind).encode(),
            Err(_) => s.clone(),
        },
        _ => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::{CueEntry, CueKind, CueType};

    #[test]
    fn cues_cleared_and_path_replaced() {
        let mut hot = CueList::empty(CueKind::Hot);
        hot.entries.push(CueEntry {
            unk_0c: 0,
            hot_cue: 1,
            unk_10: 0,
            status: 1,
            unk_16: 0,
            prev: 0xffff,
            next: 0xffff,
            cue_type: CueType::from_raw(1),
            unk_1d: 0,
            time_unit: 1000,
            time: 1234,
            loop_end: 0xffff_ffff,
            unk_28: [0; 4],
        });
        let device = AnlzFile {
            header: vec![0; 16],
            sections: vec![
                Path { unk_0c: 0, path: "/Contents/a/b/c.mp3".into() }.encode(),
                hot.encode(),
                CueList::empty(CueKind::Memory).encode(),
            ],
        };
        let local = to_local(&device, "c.mp3");
        assert_eq!(local.path().as_deref(), Some("?/c.mp3"));
        let lists = local.cue_lists();
        assert_eq!(lists.len(), 2);
        assert!(lists.iter().all(|l| l.entries.is_empty()));
        assert_eq!(lists[0].kind, CueKind::Hot);
        assert_eq!(local.sections[1], CueList::empty(CueKind::Hot).encode());
    }
}
