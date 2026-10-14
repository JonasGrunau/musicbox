//! The `.3EX` file: `{"embedding": {…}}` as MessagePack (`get_messagepack_from_json`
//! @0x1008a7638). rekordbox first prints the record as JSON text with doubles at 15 decimal
//! places, then parses that text and packs it, so every double passes through [`json_double`]
//! and is stored as float32 when that holds it exactly, else as float64.

/// A MessagePack value as the `.3EX` files use them; maps keep their order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Map(Vec<(String, Value)>),
}

/// A double after the JSON text step: printed with 15 decimal places (trailing zeros dropped,
/// which keeps the value) and read back.
pub fn json_double(v: f64) -> f64 {
    if !v.is_finite() {
        return v;
    }
    format!("{v:.15}").parse().unwrap_or(v)
}

impl Value {
    /// A double as it ends up in the file (see [`json_double`]).
    pub fn double(v: f64) -> Self {
        Value::Float(json_double(v))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(f) => Some(f),
            Value::Int(i) => Some(i as f64),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(i) => Some(i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Packs `v` with the smallest encodings (positive/negative fixint, fixstr, fixarray, fixmap,
/// then the 8/16/32-bit forms), big-endian.
pub fn encode(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    put(&mut out, v);
    out
}

fn put_len(out: &mut Vec<u8>, n: usize, fix: u8, fix_max: usize, b8: Option<u8>, b16: u8, b32: u8) {
    if n <= fix_max {
        out.push(fix | n as u8);
    } else if let (Some(t), true) = (b8, n <= 0xff) {
        out.extend([t, n as u8]);
    } else if n <= 0xffff {
        out.push(b16);
        out.extend((n as u16).to_be_bytes());
    } else {
        out.push(b32);
        out.extend((n as u32).to_be_bytes());
    }
}

fn put(out: &mut Vec<u8>, v: &Value) {
    match v {
        &Value::Int(i) if i >= 0 => {
            let u = i as u64;
            if u < 0x80 {
                out.push(u as u8);
            } else if u <= 0xff {
                out.extend([0xcc, u as u8]);
            } else if u <= 0xffff {
                out.push(0xcd);
                out.extend((u as u16).to_be_bytes());
            } else if u <= 0xffff_ffff {
                out.push(0xce);
                out.extend((u as u32).to_be_bytes());
            } else {
                out.push(0xcf);
                out.extend(u.to_be_bytes());
            }
        }
        &Value::Int(i) => {
            if i >= -32 {
                out.push(i as u8);
            } else if i >= i64::from(i8::MIN) {
                out.extend([0xd0, i as u8]);
            } else if i >= i64::from(i16::MIN) {
                out.push(0xd1);
                out.extend((i as i16).to_be_bytes());
            } else if i >= i64::from(i32::MIN) {
                out.push(0xd2);
                out.extend((i as i32).to_be_bytes());
            } else {
                out.push(0xd3);
                out.extend(i.to_be_bytes());
            }
        }
        &Value::Float(f) => {
            let in_range = f >= f64::from(f32::MIN) && f <= f64::from(f32::MAX);
            if in_range && f64::from(f as f32) == f {
                out.push(0xca);
                out.extend((f as f32).to_bits().to_be_bytes());
            } else {
                out.push(0xcb);
                out.extend(f.to_bits().to_be_bytes());
            }
        }
        Value::Str(s) => {
            put_len(out, s.len(), 0xa0, 31, Some(0xd9), 0xda, 0xdb);
            out.extend(s.as_bytes());
        }
        Value::Array(a) => {
            put_len(out, a.len(), 0x90, 15, None, 0xdc, 0xdd);
            for x in a {
                put(out, x);
            }
        }
        Value::Map(m) => {
            put_len(out, m.len(), 0x80, 15, None, 0xde, 0xdf);
            for (k, x) in m {
                put(out, &Value::Str(k.clone()));
                put(out, x);
            }
        }
    }
}

/// Reads one value covering all of `b`.
pub fn decode(b: &[u8]) -> Result<Value, String> {
    let mut at = 0;
    let v = get(b, &mut at)?;
    if at != b.len() {
        return Err(format!("{} trailing bytes", b.len() - at));
    }
    Ok(v)
}

fn take<'a>(b: &'a [u8], at: &mut usize, n: usize) -> Result<&'a [u8], String> {
    let s = b.get(*at..*at + n).ok_or("truncated")?;
    *at += n;
    Ok(s)
}

fn be(b: &[u8], at: &mut usize, n: usize) -> Result<u64, String> {
    Ok(take(b, at, n)?.iter().fold(0u64, |a, &x| a << 8 | u64::from(x)))
}

fn get(b: &[u8], at: &mut usize) -> Result<Value, String> {
    let t = take(b, at, 1)?[0];
    let n = match t {
        0x00..=0x7f => return Ok(Value::Int(i64::from(t))),
        0xe0..=0xff => return Ok(Value::Int(i64::from(t as i8))),
        0xcc..=0xcf => return Ok(Value::Int(be(b, at, 1 << (t - 0xcc))? as i64)),
        0xd0 => return Ok(Value::Int(i64::from(be(b, at, 1)? as u8 as i8))),
        0xd1 => return Ok(Value::Int(i64::from(be(b, at, 2)? as u16 as i16))),
        0xd2 => return Ok(Value::Int(i64::from(be(b, at, 4)? as u32 as i32))),
        0xd3 => return Ok(Value::Int(be(b, at, 8)? as i64)),
        0xca => return Ok(Value::Float(f64::from(f32::from_bits(be(b, at, 4)? as u32)))),
        0xcb => return Ok(Value::Float(f64::from_bits(be(b, at, 8)?))),
        0xa0..=0xbf => usize::from(t & 0x1f),
        0xd9 => be(b, at, 1)? as usize,
        0xda => be(b, at, 2)? as usize,
        0xdb => be(b, at, 4)? as usize,
        0x90..=0x9f => return array(b, at, usize::from(t & 0x0f)),
        0xdc => {
            let n = be(b, at, 2)? as usize;
            return array(b, at, n);
        }
        0xdd => {
            let n = be(b, at, 4)? as usize;
            return array(b, at, n);
        }
        0x80..=0x8f => return map(b, at, usize::from(t & 0x0f)),
        0xde => {
            let n = be(b, at, 2)? as usize;
            return map(b, at, n);
        }
        0xdf => {
            let n = be(b, at, 4)? as usize;
            return map(b, at, n);
        }
        _ => return Err(format!("unsupported type byte {t:#04x}")),
    };
    let s = take(b, at, n)?;
    Ok(Value::Str(String::from_utf8(s.to_vec()).map_err(|e| e.to_string())?))
}

fn array(b: &[u8], at: &mut usize, n: usize) -> Result<Value, String> {
    (0..n).map(|_| get(b, at)).collect::<Result<_, _>>().map(Value::Array)
}

fn map(b: &[u8], at: &mut usize, n: usize) -> Result<Value, String> {
    let mut m = Vec::with_capacity(n);
    for _ in 0..n {
        let Value::Str(k) = get(b, at)? else { return Err("non-string map key".into()) };
        m.push((k, get(b, at)?));
    }
    Ok(Value::Map(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_take_the_json_step() {
        // a float that 15 decimals cannot hold exactly comes back one ulp off, as in the files
        let f = f32::from_bits(0x3e94_d42c);
        assert_eq!(json_double(f64::from(f)).to_bits(), 0x3fd2_9a85_7fff_ffff);
        assert_eq!(encode(&Value::double(0.0)), [0xca, 0, 0, 0, 0]);
        assert_eq!(encode(&Value::double(140.0)), [0xca, 0x43, 0x0c, 0, 0]);
    }

    #[test]
    fn round_trips() {
        let v = Value::Map(vec![
            ("a".into(), Value::Int(-1)),
            ("b".into(), Value::Array((0..64).map(|i| Value::Int(i * 1000)).collect())),
            ("c".into(), Value::Str("x".repeat(40))),
            ("d".into(), Value::double(0.1)),
        ]);
        let b = encode(&v);
        assert_eq!(b[0], 0x84);
        assert_eq!(decode(&b).unwrap(), v);
    }
}
