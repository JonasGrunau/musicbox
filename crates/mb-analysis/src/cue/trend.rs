//! The personal CUE tendency (`cue_personal_trend.json`, `docs/re/analysis-cue.md` § 4): how
//! often the user's cues sit on a phrase boundary between two structures, per mood, and in which
//! third of the track. rekordbox builds it from the tracks of the [CUE Analysis Playlist]
//! (`PersonalTrendAnalyzer`) and adds it to the Auto CUE scores (`getPersonalTrendScoreMap`).

/// Phrase kind → structure, per mood (`PersonalTrendAnalyzer::structure_value_matrix`, static
/// initializer @0x10191c1d4): 0 none, 1 intro, 2 up / verse, 3 down / bridge, 4 chorus, 5 outro.
const STRUCTURE: [&[usize]; 3] = [&[0, 1, 2, 3, 0, 4, 5], &[0, 1, 2, 2, 2, 2, 2, 2, 3, 4, 5], &[0, 1, 2, 2, 2, 2, 2, 2, 3, 4, 5]];
/// `vector_of_active_beat_to_next_structure`: the beats before the next boundary that count. Only
/// the boundary itself; the matrix has room for six (36 rows = 6 offsets × 6 structures).
const ACTIVE: [i32; 1] = [0];
pub const MOODS: usize = 3;
pub const ROWS: usize = 36;
pub const COLS: usize = 6;

/// The tendency: per mood a 36 × 6 matrix (row offset + 6·previous structure, column next
/// structure) and the distribution over the thirds of a track, each scaled to 0…1.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    pub matrix: [[[f32; COLS]; ROWS]; MOODS],
    pub position: [f32; 3],
}

impl Default for Trend {
    fn default() -> Self {
        Self { matrix: [[[0.0; COLS]; ROWS]; MOODS], position: [0.0; 3] }
    }
}

/// One track of the trend analysis, as `DatabaseMediator::StartPersonalTrend` @0x1014978e4
/// collects it: the PQTZ times, the `InMsec` of its cues (hot and memory, after rekordbox's
/// duplicate clean-up), the length in whole seconds and the stored PSSI.
#[derive(Debug, Clone, Default)]
pub struct TrendTrack {
    pub beats_ms: Vec<u32>,
    pub cues_ms: Vec<u32>,
    pub length: i32,
    pub mood: i32,
    /// Phrase start beats (1-based) and kinds.
    pub starts: Vec<i32>,
    pub kinds: Vec<i32>,
}

impl Trend {
    /// All zeros, the file rekordbox writes when it has none.
    pub fn is_zero(&self) -> bool {
        self.position.iter().all(|&v| v == 0.0) && self.matrix.iter().flatten().flatten().all(|&v| v == 0.0)
    }

    /// Reads `cue_personal_trend.json`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let num = |v: &serde_json::Value| v.as_f64().map(|x| x as f32).ok_or_else(|| "number expected".to_owned());
        let mut t = Self::default();
        let m = v["personal_trend_matrix"].as_array().ok_or("personal_trend_matrix missing")?;
        if m.len() != MOODS {
            return Err(format!("{} moods", m.len()));
        }
        for (mood, rows) in m.iter().enumerate() {
            let rows = rows.as_array().filter(|r| r.len() == ROWS).ok_or("36 rows expected")?;
            for (r, cols) in rows.iter().enumerate() {
                let cols = cols.as_array().filter(|c| c.len() == COLS).ok_or("6 columns expected")?;
                for (c, x) in cols.iter().enumerate() {
                    t.matrix[mood][r][c] = num(x)?;
                }
            }
        }
        let p = v["personal_trend_position_distribution"].as_array().filter(|p| p.len() == 3).ok_or("3 positions expected")?;
        for (i, x) in p.iter().enumerate() {
            t.position[i] = num(x)?;
        }
        Ok(t)
    }

    /// The file as rekordbox writes it: nlohmann's `dump(4)` of an ordered object, written by
    /// `juce::File::replaceWithText` with CRLF line ends and no final line end.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\n    \"personal_trend_matrix\": [\n");
        for (mi, mood) in self.matrix.iter().enumerate() {
            s.push_str("        [\n");
            for (ri, row) in mood.iter().enumerate() {
                s.push_str("            [\n");
                for (ci, &v) in row.iter().enumerate() {
                    s.push_str("                ");
                    s.push_str(&json_number(v));
                    s.push_str(if ci + 1 < COLS { ",\n" } else { "\n" });
                }
                s.push_str(if ri + 1 < ROWS { "            ],\n" } else { "            ]\n" });
            }
            s.push_str(if mi + 1 < MOODS { "        ],\n" } else { "        ]\n" });
        }
        s.push_str("    ],\n    \"personal_trend_position_distribution\": [\n");
        for (i, &v) in self.position.iter().enumerate() {
            s.push_str("        ");
            s.push_str(&json_number(v));
            s.push_str(if i + 1 < 3 { ",\n" } else { "\n" });
        }
        s.push_str("    ]\n}");
        s.replace('\n', "\r\n")
    }

    /// `CAnalyzerIF::AnalyzePersonalTrend` @0x1016332c8: a fresh count over `tracks`
    /// (`analyzeTrack` @0x1019178bc each), then `updatePersonalTrend` @0x101918a70.
    pub fn analyze(tracks: &[TrendTrack]) -> Self {
        let mut counts = [[[0i32; COLS]; ROWS]; MOODS];
        let mut thirds = [0i32; 3];
        for t in tracks {
            count_track(t, &mut counts, &mut thirds);
        }
        let mut out = Self::default();
        // `MinMaxNormalization` @0x1019186b8, per mood
        for (m, mood) in counts.iter().enumerate() {
            let (mut lo, mut hi) = (0i32, 0i32);
            for &v in mood.iter().flatten() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            if hi - lo > 0 {
                let (base, r) = (lo as f32, 1.0 / (hi - lo) as f32);
                for (r_i, row) in mood.iter().enumerate() {
                    for (c, &v) in row.iter().enumerate() {
                        out.matrix[m][r_i][c] = (v as f32 - base) * r;
                    }
                }
            }
        }
        let (mut lo, mut hi) = (0f32, 0f32);
        for &v in &thirds {
            lo = (v as f32).min(lo);
            hi = (v as f32).max(hi);
        }
        if hi - lo > 0.0 {
            let r = 1.0 / (hi - lo);
            for (o, &v) in out.position.iter_mut().zip(&thirds) {
                *o = (v as f32 - lo) * r;
            }
        }
        out
    }

    /// `ScoreCalculator::getPersonalTrendScoreMap` @0x101919a68: a score for each phrase boundary
    /// (start − 1 − offset, and the end beat), kept when positive, by 0-based beat index.
    /// `beats_ms` are the grid's times, `length` the track's whole seconds.
    pub(super) fn boundary_scores(&self, mood: i32, starts: &[i32], kinds: &[i32], end_beat: i32, beats_ms: &[u32], length: i32) -> Vec<(i32, f32)> {
        let n = starts.len().min(kinds.len());
        let mut out: Vec<(i32, f32)> = Vec::new();
        if n == 0 || !(1..=MOODS as i32).contains(&mood) {
            return out;
        }
        let m = mood as usize - 1;
        let s = |kind: i32| STRUCTURE[m].get(kind as usize).copied();
        let last_ms = beats_ms.last().map_or(0, |&v| v as i32);
        for b in 0..=n {
            for (oi, &off) in ACTIVE.iter().enumerate() {
                let (beat, prev, next) = if b == 0 {
                    let beat = starts[0] - 1 - off;
                    if beat < 0 {
                        continue;
                    }
                    (beat, 0, kinds[0])
                } else {
                    let beat = if b == n { end_beat } else { starts[b] } - 1 - off;
                    if beat < starts[b - 1] - 1 {
                        continue;
                    }
                    (beat, kinds[b - 1], if b == n { 0 } else { kinds[b] })
                };
                let (Some(sp), Some(sn), Some(&ms)) = (s(prev), s(next), beats_ms.get(beat as usize)) else { continue };
                let ms = ms as i32;
                let third = if ms < last_ms {
                    if ms > 0 && length != 0 { ms.wrapping_mul(3) / length.wrapping_mul(1000) } else { 0 }
                } else {
                    2
                };
                let Some(&p) = self.position.get(third as usize) else { continue };
                let v = p.mul_add(0.5, self.matrix[m][oi + ACTIVE.len() * sp][sn]);
                if v > 0.0 && !out.iter().any(|e| e.0 == beat) {
                    out.push((beat, v));
                }
            }
        }
        out.sort_by_key(|e| e.0);
        out
    }
}

/// `analyzeTrack` @0x1019178bc for one track.
fn count_track(t: &TrendTrack, counts: &mut [[[i32; COLS]; ROWS]; MOODS], thirds: &mut [i32; 3]) {
    let n = t.starts.len().min(t.kinds.len());
    if !(30..=1800).contains(&t.length) || t.beats_ms.is_empty() || n == 0 || t.cues_ms.is_empty() || !(1..=MOODS as i32).contains(&t.mood) {
        return;
    }
    let m = t.mood as usize - 1;
    // boundaries: every start − 1, then the end beat − 1; the trend pass reads no end beat (0)
    let mut bounds: Vec<u32> = t.starts[..n].iter().map(|&s| (s - 1) as u32).collect();
    bounds.push(u32::MAX);
    // each cue on its nearest beat (the earlier one when equal), kept within 100 ms
    let mut kept: Vec<(u32, u32)> = Vec::new();
    for &c in &t.cues_ms {
        let i = t.beats_ms.partition_point(|&b| b < c);
        let idx = if i == 0 {
            0
        } else if i >= t.beats_ms.len() {
            i - 1
        } else if c.abs_diff(t.beats_ms[i - 1]) as i32 > c.abs_diff(t.beats_ms[i]) as i32 {
            i
        } else {
            i - 1
        };
        if c.abs_diff(t.beats_ms[idx]) <= 100 {
            kept.push((idx as u32, c));
        }
    }
    // at least half the cues on beats
    if kept.is_empty() || (kept.len() as u32 as f32) / (t.cues_ms.len() as u32 as f32) < 0.5 {
        return;
    }
    let len_ms = t.length.wrapping_mul(1000);
    let mut seen: Vec<u32> = Vec::new();
    let last = bounds.len() - 1;
    for &(b, ms) in &kept {
        if seen.contains(&b) {
            continue;
        }
        let j = bounds.partition_point(|&x| x < b);
        let (prev, next, d) = if j == 0 {
            (0, t.kinds[0], bounds[0].wrapping_sub(b) as i32)
        } else if j < last {
            (t.kinds[j - 1], t.kinds[j], bounds[j].wrapping_sub(b) as i32)
        } else if j == last {
            (t.kinds[j - 1], 0, bounds[last].wrapping_sub(b) as i32)
        } else {
            (0, 0, bounds[j - 1].wrapping_sub(b) as i32)
        };
        if d < 0 {
            continue;
        }
        let Some(oi) = ACTIVE.iter().position(|&o| o == d) else { continue };
        let (Some(&sp), Some(&sn)) = (STRUCTURE[m].get(prev as usize), STRUCTURE[m].get(next as usize)) else { continue };
        counts[m][oi + ACTIVE.len() * sp][sn] += 1;
        let third = if ms as i32 >= len_ms {
            2
        } else if ms == 0 {
            0
        } else {
            (ms.wrapping_mul(3) / len_ms as u32) as usize
        };
        if let Some(c) = thirds.get_mut(third) {
            *c += 1;
        }
        seen.push(b);
    }
}

/// nlohmann's number output for a float widened to double: the shortest round-trip digits, as
/// `x.0` for integers, fixed between 1e-4 and 1e15, else `d.ddde±XX` (`format_buffer`).
fn json_number(v: f32) -> String {
    let x = f64::from(v);
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let sci = format!("{:e}", x.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let sign = if x < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 15 {
        format!("{digits}{}.0", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 15 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -4 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let m = if k == 1 { digits.clone() } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        format!("{m}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_as_nlohmann_writes_them() {
        assert_eq!(json_number(0.0), "0.0");
        assert_eq!(json_number(1.0), "1.0");
        assert_eq!(json_number(0.5), "0.5");
        assert_eq!(json_number(1.0 / 3.0), "0.3333333432674408");
        assert_eq!(json_number(0.001), "0.0010000000474974513");
        assert_eq!(json_number(0.00001), "9.999999747378752e-06");
        assert_eq!(json_number(120.0), "120.0");
    }

    #[test]
    fn zeros_round_trip_and_match_the_fixture_layout() {
        let z = Trend::default();
        let text = z.to_json();
        assert!(text.starts_with("{\r\n    \"personal_trend_matrix\": [\r\n        [\r\n            [\r\n                0.0,\r\n"));
        assert!(text.ends_with("        0.0\r\n    ]\r\n}"));
        assert_eq!(Trend::parse(&text).unwrap(), z);
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle-cue/A1/cue_personal_trend.json");
        if let Ok(theirs) = std::fs::read_to_string(fixture) {
            assert_eq!(text, theirs);
        }
    }

    fn track(cues: &[u32]) -> TrendTrack {
        // 4 phrases of 32 beats at 500 ms, mood 1: intro, up, down, chorus
        TrendTrack {
            beats_ms: (0..160).map(|i| i * 500).collect(),
            cues_ms: cues.to_vec(),
            length: 80,
            mood: 1,
            starts: vec![1, 33, 65, 97],
            kinds: vec![1, 2, 3, 5],
        }
    }

    #[test]
    fn cues_on_phrase_starts_count_by_structure_and_third() {
        // beat 32 (up after intro), beat 64 (down after up), beat 64 again (deduplicated), one
        // cue 40 ms off beat 96 (chorus after down), one off any beat (ignored, 4 of 5 kept)
        let t = Trend::analyze(&[track(&[16_000, 32_000, 32_010, 48_040, 10_250])]);
        // rows: previous structure (intro 1, up 2, down 3); columns: next (up 2, down 3, chorus 4)
        assert_eq!(t.matrix[0][1][2], 1.0);
        assert_eq!(t.matrix[0][2][3], 1.0);
        assert_eq!(t.matrix[0][3][4], 1.0);
        assert_eq!(t.matrix[0].iter().flatten().filter(|&&v| v != 0.0).count(), 3);
        // 16 s, 32 s, 48 s of 80 s: thirds 0, 1, 1
        assert_eq!(t.position, [0.5, 1.0, 0.0]);
        // fewer than half the cues on beats: nothing
        assert!(Trend::analyze(&[track(&[16_000, 10_250, 11_300, 12_200])]).is_zero());
        // too short
        assert!(Trend::analyze(&[TrendTrack { length: 29, ..track(&[16_000]) }]).is_zero());
    }

    #[test]
    fn boundary_scores_follow_the_matrix() {
        let t = Trend::analyze(&[track(&[16_000, 32_000, 48_000])]);
        let tr = track(&[]);
        let s = t.boundary_scores(1, &tr.starts, &tr.kinds, 160, &tr.beats_ms, 80);
        // beats 32, 64, 96 carry matrix 1.0 plus half their third's share; the first start has
        // only the position share; the end beat (last third, share 0) none, so it is left out
        assert_eq!(s.iter().map(|e| e.0).collect::<Vec<_>>(), [0, 32, 64, 96]);
        let at = |b: i32| s.iter().find(|e| e.0 == b).unwrap().1;
        assert_eq!(at(32), 1.0 + 0.5 * t.position[0]);
        assert_eq!(at(64), 1.0 + 0.5 * t.position[1]);
        assert_eq!(at(0), 0.5 * t.position[0]);
    }
}
