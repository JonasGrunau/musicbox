//! `BeatAnalyzer_1_0::Key_judge_with_keys` @0x100c2e5ac with `KeyJudge_step_1` @0x100c2ee50 and
//! `KeyJudge_step_2toFinal` @0x100c30410 (`analysis-key.md` § 7): the final key from the
//! whole-track pair and the per-segment estimates.
//!
//! rekordbox keeps the judgement's state in globals (`top5`, `scores`, `rdp`, …), reset on every
//! call; here they are the fields of [`Judge`]. The control flow follows the binary's branch for
//! branch, so some paths read oddly.

/// The relations `KeyJudge_step_1` scores: offset, wrap threshold and weight, for major
/// (@0x1037afc20, @0x1037afc08) and minor keys (@0x1037afbf0, @0x1037afbd8); weights @0x1037afc38.
const REL_MAJOR: [(i32, i32); 6] = [(12, 24), (7, 11), (5, 11), (21, 23), (16, 23), (14, 23)];
const REL_MINOR: [(i32, i32); 6] = [(-12, 24), (7, 23), (5, 23), (-9, 11), (-2, 11), (-4, 11)];
const REL_WEIGHT: [i32; 6] = [1, 2, 2, 3, 3, 3];

/// The `i`-th related key of `key` (parallel, dominant, subdominant, relative, …).
fn related(key: i32, i: usize) -> i32 {
    let (off, thr) = if key > 11 { REL_MINOR[i] } else { REL_MAJOR[i] };
    let r = key + off;
    if r > thr { r - 12 } else { r }
}

/// The relative major/minor of `key`.
fn relative(key: i32) -> i32 {
    related(key, 3)
}

#[derive(Debug, Default)]
struct Judge {
    key1_current: i32,
    m_bpm: f32,
    /// `top5`: keys 0..5, then their counts.
    keys: [i32; 5],
    counts: [i32; 5],
    scores: [i32; 3],
    scores5: [i32; 3],
    /// `scores_c` values: the three best keys and the whole-track key.
    sc3: [i32; 4],
    key1: i32,
    key2: i32,
    key3: i32,
    key_no1: i32,
    key_no2: i32,
    p_cnt12: i32,
    p_cnt21: i32,
    p_sc_add_1: i32,
    p_sc_add_2: i32,
    rdp: f32,
    top12r: f32,
    top13r: f32,
    top23r: f32,
    r_t1all: f32,
    r_t12all: f32,
    minor1: bool,
    minor2: bool,
    doshu12: bool,
    doshu13: bool,
    heiko12: bool,
    heiko13: bool,
    scores_total_max: i32,
    scores_total_max_v: f32,
    scores_total_2nd_v: f32,
    r_ks12: f32,
    r_ks13: f32,
    r_ks21: f32,
    pre_final_0: i32,
    pre_final: i32,
    pre_judge_t: i32,
    final_key: i32,
    judge_type: i32,
}

/// `Key_judge_with_keys(key1, key2, nseg, segments, keys, bpm)`: `keys[i] = (key1, key2)` of
/// segment `i`. Returns the key code. (`key2` only feeds state the binary overwrites.)
pub fn judge(key1: i32, _key2: i32, segments: bool, keys: &[(i32, i32)], bpm: f32) -> i32 {
    let mut j = Judge {
        key1_current: key1,
        m_bpm: bpm,
        ..Default::default()
    };
    let n = keys.len();
    if n < 9 || !segments {
        // `new_algo_skip`: only the final check, on an empty state.
        j.final_key = key1;
        return j.finish();
    }
    let k1: Vec<i32> = keys.iter().map(|k| k.0).collect();
    j.top5(&k1);
    j.r_t1all = j.counts[0] as f32 / n as f32;
    j.r_t12all = (j.counts[1] as f32 + j.counts[0] as f32) / n as f32;
    j.step1(&k1);
    j.step2()
}

/// [`judge`] plus a dump of the judgement state (development aid, same layout as the harness).
#[doc(hidden)]
pub fn judge_trace(key1: i32, segments: bool, keys: &[(i32, i32)], bpm: f32) -> (i32, String) {
    let mut j = Judge {
        key1_current: key1,
        m_bpm: bpm,
        ..Default::default()
    };
    let n = keys.len();
    let r = if n < 9 || !segments {
        j.final_key = key1;
        j.finish()
    } else {
        let k1: Vec<i32> = keys.iter().map(|k| k.0).collect();
        j.top5(&k1);
        j.r_t1all = j.counts[0] as f32 / n as f32;
        j.r_t12all = (j.counts[1] as f32 + j.counts[0] as f32) / n as f32;
        j.step1(&k1);
        j.step2()
    };
    let k = j.keys;
    let c = j.counts;
    let s = format!(
        "S keys {} {} {} {} {} counts {} {} {} {} {} sc {} {} {} s5 {} {} {} sc3 {} {} {} {} rdp {} t12 {} t13 {} t23 {} jt {} pf {} pf0 {} pjt {} stm {} kn {} {} ks {} {} {} mv {} 2v {}",
        k[0],
        k[1],
        k[2],
        k[3],
        k[4],
        c[0],
        c[1],
        c[2],
        c[3],
        c[4],
        j.scores[0],
        j.scores[1],
        j.scores[2],
        j.scores5[0],
        j.scores5[1],
        j.scores5[2],
        j.sc3[0],
        j.sc3[1],
        j.sc3[2],
        j.sc3[3],
        j.rdp,
        j.top12r,
        j.top13r,
        j.top23r,
        j.judge_type,
        j.pre_final,
        j.pre_final_0,
        j.pre_judge_t,
        j.scores_total_max,
        j.key_no1,
        j.key_no2,
        j.r_ks12,
        j.r_ks13,
        j.r_ks21,
        j.scores_total_max_v,
        j.scores_total_2nd_v
    );
    (r, s)
}

impl Judge {
    /// The five most frequent segment keys, merged with their relatives when the whole-track key
    /// is not the most frequent one.
    fn top5(&mut self, k1: &[i32]) {
        let mut hist = [0i32; 24];
        for &k in k1 {
            hist[k as usize] += 1;
        }
        // local_a0: 7 keys, then 7 counts (30 for an empty bin).
        let mut a = [0i32; 14];
        for i in 0..7 {
            let mut best = 0usize;
            for k in 1..24 {
                if hist[k] > hist[best] {
                    best = k;
                }
            }
            a[i] = best as i32;
            a[i + 7] = if hist[best] != 0 { hist[best] } else { 30 };
            hist[best] = 0;
        }
        let count = |key: i32| k1.iter().filter(|&&k| k == key).count() as i32;
        // Register-for-register as the binary.
        let (mut u21, mut u16, mut u14, mut u12, mut u17) = (a[0], a[1], a[2], a[3], a[4]);
        let (mut u8_, mut u32_, mut u20, mut u19, mut u31) = (a[7], a[8], a[9], a[10], a[11]);
        if self.key1_current != a[0] {
            u16 = a[6];
            let r = relative(a[0]);
            let u25 = a[7] + count(r);
            let mut key0 = a[1];
            let mut u15 = a[4];
            let mut u18 = a[11];
            u17 = a[5];
            let mut u7 = a[12];
            let mut u22 = a[13];
            if r == a[1] {
                key0 = a[2];
                u14 = a[3];
                u12 = a[4];
                u15 = a[5];
                u32_ = a[9];
                u20 = a[10];
                u19 = a[11];
                u18 = a[12];
                u17 = u16;
                u7 = a[13];
                u22 = 0;
                u16 = 30;
            }
            if r == u14 {
                u14 = u12;
                u20 = u19;
                u12 = u15;
                u19 = u18;
                u18 = u7;
                u7 = u22;
                u22 = 0;
                u15 = u17;
                u17 = u16;
                u16 = 30;
            }
            let mut i26 = u22;
            u31 = u7;
            let mut u1 = u16;
            if r == u12 {
                u19 = u18;
                i26 = 0;
                u31 = u22;
                u18 = u7;
                u12 = u15;
                u1 = 30;
                u15 = u17;
                u17 = u16;
            }
            let mut i27 = i26;
            u16 = u1;
            if r == u15 {
                u16 = 30;
                i27 = 0;
                u18 = u31;
                u31 = i26;
                u15 = u17;
                u17 = u1;
            }
            let key_chk = relative(key0);
            u32_ += count(key_chk);
            let mut i26 = i27;
            u8_ = u16;
            if key_chk == u14 {
                u8_ = 30;
                i26 = 0;
                u14 = u12;
                u20 = u19;
                u19 = u18;
                u12 = u15;
                u18 = u31;
                u31 = i27;
                u15 = u17;
                u17 = u16;
            }
            u16 = u15;
            if key_chk == u12 {
                u16 = u17;
                u12 = u15;
                u19 = u18;
                u18 = u31;
                u31 = i26;
                u17 = u8_;
            }
            if key_chk != u16 {
                u31 = u18;
                u17 = u16;
            }
            u16 = key0;
            u8_ = u25;
            if u25 < u32_ {
                u16 = a[0];
                u21 = a[1];
                u8_ = u32_;
                u32_ = u25;
            }
        }
        self.keys = [u21, u16, u14, u12, u17];
        self.counts = [u8_, u32_, u20, u19, u31];
    }

    fn step1(&mut self, k1: &[i32]) {
        let [t0, t1, t2, t3, t4] = self.keys;
        let cur = self.key1_current;
        self.sc3[3] = 1;
        let tops = [t0, t1, t2];
        for (u, &key) in tops.iter().enumerate() {
            let others: Vec<i32> = (0..3).filter(|&v| v != u).map(|v| tops[v]).collect();
            for i in 0..6 {
                let rel = related(key, i);
                let w = REL_WEIGHT[i];
                if others.contains(&rel) {
                    self.scores[u] += w;
                    self.scores5[u] += w;
                } else if rel == t3 || rel == t4 {
                    self.scores5[u] += w;
                }
                if others.contains(&rel) || rel == cur {
                    self.sc3[u] += w;
                }
            }
        }
        for i in 0..6 {
            if tops.contains(&related(cur, i)) {
                self.sc3[3] += REL_WEIGHT[i];
            }
        }
        self.key1 = t0;
        self.key2 = t1;
        self.key3 = t2;
        // Entries into the three best keys.
        let (mut e0, mut e1, mut e2) = (0, 0, 0);
        match k1[0] {
            k if k == t0 => e0 = 1,
            k if k == t1 => e1 = 1,
            k if k == t2 => e2 = 1,
            _ => {}
        }
        for w in k1.windows(2) {
            let (p, x) = (w[0], w[1]);
            if x == t0 && p != t0 {
                e0 += 1;
            } else if x == t1 && p != t1 {
                e1 += 1;
            } else if x == t2 && p != t2 {
                e2 += 1;
            }
        }
        let dt2 = e1 + e0 + e2;
        // Back-and-forth patterns a → b → (not a).
        let (mut p21, mut p31, mut p12, mut p32, mut p13, mut p23) = (0, 0, 0, 0, 0, 0);
        for w in k1.windows(3) {
            if w[0] == t1 && w[1] == t0 && w[2] != t1 {
                p21 += 1;
            } else if w[0] == t2 && w[1] == t0 && w[2] != t2 {
                p31 += 1;
            }
        }
        for w in k1.windows(3) {
            if w[0] == t0 && w[1] == t1 && w[2] != t0 {
                p12 += 1;
            } else if w[0] == t2 && w[1] == t1 && w[2] != t2 {
                p32 += 1;
            }
        }
        for w in k1.windows(3) {
            if w[0] == t0 && w[1] == t2 && w[2] != t0 {
                p13 += 1;
            } else if w[0] == t1 && w[1] == t2 && w[2] != t1 {
                p23 += 1;
            }
        }
        self.p_cnt12 = p12;
        self.p_cnt21 = p21;
        let nz = |c: i32| if c == 0 { 1 } else { c } as f32;
        let [c0, c1, c2, _, _] = self.counts;
        self.key_no1 = 1;
        self.key_no2 = 2;
        if p12 as f32 / nz(c0) <= p21 as f32 / nz(c1) {
            if (p31 as f32) / nz(c2) < p13 as f32 / nz(c0) {
                self.key_no1 = 3;
                self.key_no2 = 1;
            }
        } else if p23 as f32 / nz(c1) <= p32 as f32 / nz(c2) {
            self.key_no1 = 2;
            self.key_no2 = 3;
        } else {
            self.key_no1 = 3;
        }
        self.p_sc_add_1 = p31 + p21;
        self.p_sc_add_2 = p32 + p12;
        let p_sc_add_3 = p23 + p13;
        let mut pt = self.p_sc_add_2 + self.p_sc_add_1 + p_sc_add_3;
        if pt == 0 {
            pt = 1;
        }
        self.rdp = dt2 as f32 / pt as f32;
        if c1 != 0 {
            self.top12r = c0 as f32 / c1 as f32;
        }
        if c2 != 0 {
            self.top13r = c0 as f32 / c2 as f32;
            self.top23r = c1 as f32 / c2 as f32;
        }
        self.minor1 = t0 > 11;
        self.minor2 = t1 > 11;
        let par = |k: i32| if k > 11 { k - 12 } else { k + 12 };
        self.doshu12 = par(t0) == t1;
        self.doshu13 = par(t0) == t2;
        self.heiko12 = relative(t0) == t1;
        self.heiko13 = relative(t0) == t2;
        self.decide1();
    }

    /// The pre-judgement at the end of `KeyJudge_step_1` (label for label).
    fn decide1(&mut self) {
        #[derive(Clone, Copy)]
        enum L {
            Fc24,
            Fc2c,
            Fdac,
            Fc50,
            Fcac,
            Fe10,
            Fe28,
            Fd84,
            Feb8,
            L30004,
        }
        let st1 = self.scores5[0] + self.scores[0] + self.sc3[0];
        let st2 = self.scores5[1] + self.scores[1] + self.sc3[1];
        let st3 = self.scores5[2] + self.scores[2] + self.sc3[2];
        let mut st1m = st1;
        self.scores_total_2nd_v = st1 as f32;
        let mut f30 = st2 as f32;
        let f33 = st3 as f32;
        self.r_ks21 = f30;
        let mut f31 = f33;
        if st1 != 0 {
            self.r_ks21 = f30 / self.scores_total_2nd_v;
            f31 = f33 / self.scores_total_2nd_v;
        }
        self.r_ks12 = self.scores_total_2nd_v;
        let mut f32_ = f33;
        if st2 != 0 {
            self.r_ks12 = self.scores_total_2nd_v / f30;
            f32_ = f33 / f30;
        }
        self.r_ks13 = self.scores_total_2nd_v;
        if st3 != 0 {
            self.r_ks13 = self.scores_total_2nd_v / f33;
        }
        self.scores_total_max = 1;
        self.scores_total_max_v = f33;
        let rdp = self.rdp;
        let cur = self.key1_current;
        let (mut a, mut b, mut c6) = (0i32, 0i32, self.keys[1]);
        let (mut b7, mut b8, mut b9) = (false, false, false);
        let mut l;
        if st1 < st2 {
            if st2 < st3 {
                a = 3;
                self.scores_total_max = 3;
                b8 = true;
                l = L::Fc2c;
            } else {
                b = 2;
                a = 2;
                self.scores_total_max = 2;
                self.scores_total_max_v = f30;
                if st1 < st3 {
                    l = L::Fc24;
                } else {
                    b8 = false;
                    b9 = true;
                    l = if 4.0 <= f64::from(rdp) {
                        L::Fdac
                    } else {
                        L::Fc50
                    };
                }
            }
        } else if st3 <= st1 {
            a = 1;
            b8 = true;
            self.scores_total_max_v = self.scores_total_2nd_v;
            b = a;
            l = if st2 < st3 { L::Fc24 } else { L::Fc2c };
        } else {
            b8 = false;
            b = 3;
            self.scores_total_max = 3;
            if 4.0 <= f64::from(rdp) {
                l = L::Fdac;
            } else {
                b9 = true;
                l = L::Fcac;
            }
        }
        loop {
            match l {
                L::Fc24 => {
                    a = b;
                    b8 = false;
                    f30 = f33;
                    l = L::Fc2c;
                }
                L::Fc2c => {
                    self.scores_total_2nd_v = f30;
                    b9 = false;
                    b = a;
                    l = if f64::from(rdp) < 4.0 {
                        L::Fc50
                    } else {
                        L::Fdac
                    };
                }
                L::Fdac => {
                    self.pre_judge_t = 0x65;
                    if 4.0 <= f64::from(rdp) {
                        l = L::Fe28;
                    } else {
                        b9 = false;
                        a = 4;
                        self.pre_final_0 = 4;
                        self.pre_judge_t = 0x68;
                        self.pre_final = 4;
                        b7 = true;
                        l = L::L30004;
                    }
                }
                L::Fc50 => {
                    if a == 3 {
                        l = L::Fcac;
                        continue;
                    }
                    if a != 2 {
                        b = 1;
                        l = L::Fe10;
                        continue;
                    }
                    if self.key_no1 == 2 || self.key_no2 == 2 {
                        if self.p_sc_add_1 == 0 {
                            self.p_sc_add_1 = 1;
                        }
                        if b9 && f64::from(self.r_ks21) < 1.2 {
                            let t = self.p_sc_add_2 / self.p_sc_add_1;
                            self.pre_judge_t = 0x70;
                            if f64::from(t) < 1.1 && 5 < self.p_sc_add_1 {
                                b = 2;
                                self.pre_judge_t = 0x6f;
                                l = L::Fe28;
                                continue;
                            }
                        } else {
                            self.pre_judge_t = 0x70;
                        }
                    } else {
                        b = 2;
                        self.pre_judge_t = 0x83;
                        if f64::from(self.r_ks21) <= 1.2 || self.scores5[1] <= self.scores5[0] {
                            l = L::Fe28;
                            continue;
                        }
                        self.pre_judge_t = 0x7a;
                    }
                    a = 2;
                    self.pre_final_0 = 2;
                    self.pre_final = 2;
                    b9 = false;
                    b7 = false;
                    b = 2;
                    l = L::L30004;
                    if cur == self.key1 && f64::from(self.top12r) < 1.1 {
                        c6 = 2;
                        if 0.75 < f64::from(self.r_ks12) {
                            l = L::Feb8;
                        }
                    }
                }
                L::Fcac => {
                    if 2.6 <= f64::from(rdp) {
                        b = 3;
                        l = L::Fe10;
                        continue;
                    }
                    self.pre_judge_t = 0x83;
                    if !(b9 && 1.4 < f64::from(f31)) {
                        if !(b8 && 1.4 < f64::from(f32_)) {
                            b = 3;
                            l = L::Fe28;
                            continue;
                        }
                        b8 = true;
                    }
                    b9 = false;
                    b = 3;
                    self.pre_final_0 = 3;
                    self.pre_judge_t = 0x85;
                    self.pre_final = 3;
                    if cur != self.key1 || 4.0 <= f64::from(self.top13r) {
                        b7 = false;
                        a = 3;
                        l = L::L30004;
                    } else {
                        a = 3;
                        f31 = self.r_ks13;
                        l = L::Fd84;
                    }
                }
                L::Fe10 => {
                    self.pre_judge_t = 0x83;
                    l = L::Fe28;
                }
                L::Fe28 => {
                    self.pre_final_0 = 1;
                    self.pre_final = 1;
                    let stay = cur != self.key2 || 1.1 <= f64::from(self.top12r) || {
                        c6 = b;
                        f64::from(self.r_ks21) <= 0.75
                    };
                    if stay {
                        b7 = false;
                        b9 = true;
                        a = 1;
                        l = if cur == self.key3 && f64::from(self.top13r) < 4.0 {
                            L::Fd84
                        } else {
                            L::L30004
                        };
                    } else {
                        l = L::Feb8;
                    }
                }
                L::Fd84 => {
                    b7 = false;
                    c6 = b;
                    l = if 1.6 < f64::from(f31) {
                        L::Feb8
                    } else {
                        L::L30004
                    };
                }
                L::Feb8 => {
                    b = c6;
                    b9 = false;
                    a = 4;
                    self.pre_final = 4;
                    self.pre_judge_t += 1000;
                    b7 = true;
                    l = L::L30004;
                }
                L::L30004 => break,
            }
        }
        // LAB_100c30004
        #[derive(Clone, Copy)]
        enum M {
            J3004c,
            L30050,
            L30118,
            L3011c,
            L30178,
            L30124,
            L30220,
            L301f4,
            L30334,
            End,
        }
        if self.scores_total_2nd_v == 0.0 {
            self.scores_total_2nd_v = 1.0;
        }
        let ratio = |s: &Self| s.scores_total_max_v / s.scores_total_2nd_v;
        let mut m;
        if b9 && b == 2 {
            if f64::from(ratio(self)) < 1.2 {
                a = 1;
                m = M::J3004c;
            } else if f64::from(self.top12r) < 1.1 {
                a = 2;
                self.pre_final = 2;
                self.pre_judge_t += 2000;
                m = M::J3004c;
            } else {
                a = 1;
                m = M::L30118;
            }
        } else if !b7
            || {
                a = 4;
                cur != self.key1
            }
            || b != 2
        {
            m = M::L30118;
        } else {
            if f64::from(ratio(self)) < 1.2 || 1.1 <= f64::from(self.top12r) {
                a = 4;
            } else {
                a = 2;
                self.pre_final = 2;
                self.pre_judge_t += 2000;
            }
            m = M::J3004c;
        }
        loop {
            match m {
                M::J3004c => m = if st1m != 0 { M::L3011c } else { M::L30050 },
                M::L30050 => {
                    st1m = 1;
                    m = if a != 1 { M::L30178 } else { M::L30124 };
                }
                M::L30118 => m = if st1m == 0 { M::L30050 } else { M::L3011c },
                M::L3011c => m = if a == 1 { M::L30124 } else { M::L30178 },
                M::L30178 => {
                    m = M::L30220;
                    if a == 4 {
                        b7 = true;
                        if cur == self.key1
                            && b == 2
                            && 1.2 <= f64::from(self.scores_total_max_v / (st1m as f32))
                            && f64::from(self.top12r) < 1.55
                        {
                            m = M::L301f4;
                        }
                    } else {
                        b7 = false;
                    }
                }
                M::L30124 => {
                    if b != 2 {
                        m = M::L30178;
                        continue;
                    }
                    b7 = false;
                    m = if f64::from(self.scores_total_max_v / (st1m as f32)) < 1.2
                        || 1.55 <= f64::from(self.top12r)
                    {
                        M::L30220
                    } else {
                        M::L301f4
                    };
                }
                M::L30220 => {
                    let test = |s: &Self| {
                        let t = if st1m != 0 { st2 / st1m } else { 0 };
                        2 < t
                            && f64::from(s.top12r) < 1.55
                            && 3.0 < f64::from(s.rdp)
                            && f64::from(s.rdp) <= 5.0
                            && f64::from(s.r_t1all) < 0.26
                    };
                    m = M::End;
                    if (b8 && a == 1 && test(self)) || (b7 && b8 && cur == self.key1 && test(self))
                    {
                        m = M::L30334;
                    }
                }
                M::L301f4 => {
                    m = if f64::from(rdp) <= 3.0
                        || 4.0 < f64::from(rdp)
                        || 0.25 <= f64::from(self.r_t1all)
                    {
                        M::L30220
                    } else {
                        M::L30334
                    };
                }
                M::L30334 => {
                    a = 2;
                    self.pre_final = 2;
                    self.pre_judge_t += 3000;
                    m = M::End;
                }
                M::End => break,
            }
        }
        if self.scores_total_2nd_v == 0.0 {
            self.scores_total_2nd_v = 1.0;
        }
        if ((a == 1 && b == 3) || (a == 4 && cur == self.key1 && b == 3))
            && 1.5 <= f64::from(ratio(self))
            && f64::from(self.top13r) < 4.0
            && f64::from(rdp) < 4.2
            && f64::from(self.top12r) < 1.7
        {
            self.pre_final = 3;
            self.pre_judge_t += 4000;
        }
    }

    /// `KeyJudge_step_2toFinal`.
    fn step2(&mut self) -> i32 {
        self.judge_type = 0;
        self.final_key = self.key1;
        let cur = self.key1_current;
        let top12r = self.top12r;
        let rdp = self.rdp;
        let (b2, b4);
        'l3085c: {
            if 1.8 <= f64::from(top12r) {
                b4 = false;
                b2 = true;
                self.judge_type = 0xb;
                break 'l3085c;
            }
            let (s, s5, c) = (self.scores, self.scores5, self.sc3);
            if s[0] < s[1] || s5[0] < s5[1] || c[0] < c[1] || s[0] < 5 || s5[0] < 5 || c[0] < 5 {
                if self.doshu12 {
                    self.judge_type = 0x1f;
                    if f64::from(top12r) <= 1.2 && self.pre_final_0 != 1 && self.pre_final_0 != 4 {
                        self.judge_type = 0x20;
                        self.final_key = self.key2;
                    }
                    let minor = if self.final_key != self.key2 {
                        self.minor1
                    } else {
                        self.minor2
                    };
                    let x = self.final_key + if minor { -12 } else { 12 };
                    let key_chk = if x < 25 { x } else { x - 12 };
                    if key_chk == cur
                        && !(1.25 <= f64::from(top12r) || 1.23 <= f64::from(top12r))
                        && f64::from(rdp) < 3.5
                    {
                        let keep = ((f64::from(top12r) <= 1.06
                            || f64::from(rdp) <= 2.5
                            || 3.0 <= f64::from(rdp))
                            && (f64::from(top12r) <= 1.1
                                || f64::from(rdp) <= 2.0
                                || 3.0 <= f64::from(rdp))
                            && (f64::from(top12r) <= 1.2 || f64::from(rdp) < 3.0))
                            && (1.06 < f64::from(top12r) || 2.0 <= f64::from(rdp));
                        if !keep {
                            self.judge_type = 0x21;
                            self.final_key = key_chk;
                        }
                    }
                } else {
                    if !self.doshu13 {
                        b4 = true;
                        b2 = false;
                        break 'l3085c;
                    }
                    self.judge_type = 0x29;
                    if self.key2 - self.key1 != 5
                        || self.p_cnt12 <= self.p_cnt21
                        || 2.0 <= f64::from(rdp)
                    {
                        if self.scores_total_max != 3 {
                            return self.last(cur);
                        }
                        if 1.4 < f64::from(self.scores_total_max_v / self.scores_total_2nd_v)
                            && f64::from(self.r_t12all) < 0.64
                        {
                            self.final_key = self.key3;
                            self.judge_type = 0x2b;
                        }
                    } else {
                        self.final_key = self.key2;
                        self.judge_type = 0x2a;
                    }
                }
            } else {
                self.judge_type = 0x15;
                let key_chk = relative(self.key1);
                if key_chk == cur && f64::from(top12r) < 1.39 {
                    self.judge_type = 0x16;
                    self.final_key = key_chk;
                }
            }
            b4 = false;
            b2 = true;
        }
        // LAB_100c3085c
        if !b2 && self.scores_total_max == 1 && 3.0 <= f64::from(self.top23r) {
            self.final_key = self.key1;
            self.judge_type = 0x33;
            return self.last(cur);
        }
        if b4 && self.heiko12 {
            self.final_key = self.key1;
            self.judge_type = 0x3d;
        } else if b4 && self.heiko13 {
            self.final_key = self.key1;
            self.judge_type = 0x47;
            if self.scores_total_max != 2 || f64::from(self.r_ks21) <= 1.1 {
                let c0 = self.counts[0] as f32;
                let c3 = self.counts[3] as f32;
                if relative(self.key2) == self.keys[3] {
                    if 1.5 <= f64::from(top12r) || 2.1 <= f64::from(c0 / c3) {
                        return self.last(cur);
                    }
                    self.final_key = self.key2;
                    self.judge_type = 0x49;
                } else {
                    let x = self.key2 + if self.minor2 { -12 } else { 12 };
                    let key_chk = if x < 25 { x } else { x - 12 };
                    if key_chk != self.keys[3]
                        || 1.2 <= f64::from(top12r)
                        || 2.0 <= f64::from(c0 / c3)
                    {
                        return self.last(cur);
                    }
                    self.final_key = self.key2;
                    self.judge_type = 0x4a;
                }
            } else {
                self.final_key = self.key2;
                self.judge_type = 0x48;
            }
        } else if b4 {
            let c0 = self.counts[0] as f32;
            let c4 = self.counts[4] as f32;
            if 0.6 <= f64::from(self.r_ks12)
                && 0.64 <= f64::from(self.r_t12all)
                && 8.4 < f64::from(c0 / c4)
                && f64::from(rdp) < 2.5
            {
                self.final_key = self.key1;
                self.judge_type = 0x30d;
                return self.last(cur);
            }
            let (s, s5, c) = (self.scores, self.scores5, self.sc3);
            if s[1] <= s[0]
                || s5[1] <= s5[0]
                || c[1] <= c[0]
                || s[1] < s[2]
                || s5[1] < s5[2]
                || c[1] < c[2]
            {
                if self.scores_total_2nd_v == 0.0 {
                    self.scores_total_2nd_v = 1.0;
                }
                if self.key3 == cur
                    && self.scores_total_max == 3
                    && 1.25 < f64::from(self.scores_total_max_v / self.scores_total_2nd_v)
                    && f64::from(self.top13r) < 2.0
                    && f64::from(rdp) < 6.5
                {
                    self.final_key = self.key3;
                    self.judge_type = 0x5d;
                    return self.last(cur);
                }
                self.judge_type = self.pre_judge_t;
                self.final_key = match self.pre_final {
                    4 => cur,
                    3 => self.key3,
                    2 => self.key2,
                    _ => self.key1,
                };
                return self.last(cur);
            }
            self.final_key = self.key2;
            self.judge_type = 0x52;
            if 1.6 <= f64::from(rdp)
                || f64::from(top12r) <= 1.2
                || self.key_no1 != 1
                || f64::from(self.r_ks12) < 0.5
            {
                if !(2.7 <= f64::from(rdp)
                    || f64::from(top12r) <= 1.3
                    || f64::from(self.r_ks12) <= 0.7
                    || f64::from(self.r_ks13) <= 1.0)
                {
                    self.final_key = self.key1;
                    self.judge_type = 0x54;
                }
            } else {
                self.final_key = self.key1;
                self.judge_type = 0x53;
            }
        }
        self.last(cur)
    }

    /// LAB_100c308c4: the last chance for the whole-track key.
    fn last(&mut self, cur: i32) -> i32 {
        self.finish_with(cur)
    }

    fn finish(&mut self) -> i32 {
        let cur = self.key1_current;
        self.finish_with(cur)
    }

    fn finish_with(&mut self, cur: i32) -> i32 {
        let c0 = self.counts[0];
        let ratio = if self.counts[4] != 0 {
            c0 / self.counts[4]
        } else {
            c0
        };
        let in_top = self.keys.contains(&cur);
        let s = self.sc3;
        let rdp = self.rdp;
        let top12r = self.top12r;
        let weak = !in_top
            || s[3] <= s[0]
            || s[3] <= s[1]
            || s[3] <= s[2]
            || 3.8 < f64::from(rdp)
            || 1.47 < f64::from(top12r)
            || 0x16 < ratio
            || (121.0 <= f64::from(self.m_bpm) && f64::from(self.m_bpm) <= 128.0);
        if weak {
            if self.judge_type == 0x52 {
                if 4.0 < f64::from(rdp) && ratio < 4 {
                    return cur;
                }
            } else if self.judge_type == 0xb
                && 4.1 < f64::from(rdp)
                && (f64::from(top12r) <= 2.0 || 3.0 < f64::from(top12r))
            {
                return cur;
            }
        } else if !matches!(self.judge_type, 0x33 | 0x54 | 0x70 | 0x1f) {
            return cur;
        }
        self.final_key
    }
}
