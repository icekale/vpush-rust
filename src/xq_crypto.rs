//! 雪球注册用的 SM2 协商、SM3 和 SM4/CBC。算法与 `app/xq_crypto.py` 对齐。

use std::sync::OnceLock;

use num_bigint::BigUint;

fn hex_int(s: &str) -> BigUint {
    BigUint::parse_bytes(s.as_bytes(), 16).expect("hex int")
}

fn p() -> &'static BigUint {
    static V: OnceLock<BigUint> = OnceLock::new();
    V.get_or_init(|| hex_int("FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF"))
}
fn a() -> &'static BigUint {
    static V: OnceLock<BigUint> = OnceLock::new();
    V.get_or_init(|| hex_int("FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFC"))
}
fn n() -> &'static BigUint {
    static V: OnceLock<BigUint> = OnceLock::new();
    V.get_or_init(|| hex_int("FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123"))
}
fn g() -> &'static (BigUint, BigUint) {
    static V: OnceLock<(BigUint, BigUint)> = OnceLock::new();
    V.get_or_init(|| {
        (
            hex_int("32C4AE2C1F1981195F9904466A39C9948FE30BBFF2660BE1715A4589334C74C7"),
            hex_int("BC3736A2F4F6779C59BDCEE36B692153D0A9877CC62A474002DF32E52139F0A0"),
        )
    })
}

fn inv(x: &BigUint) -> BigUint {
    x.modpow(&(p() - 2u32), p())
}

fn point_add(
    p1: Option<&(BigUint, BigUint)>,
    p2: Option<&(BigUint, BigUint)>,
) -> Option<(BigUint, BigUint)> {
    let (Some((x1, y1)), Some((x2, y2))) = (p1, p2) else {
        return p1.or(p2).cloned();
    };
    if x1 == x2 && (y1 + y2) % p() == BigUint::from(0u32) {
        return None;
    }
    let lam = if x1 == x2 && y1 == y2 {
        (BigUint::from(3u32) * x1 * x1 + a()) * inv(&(BigUint::from(2u32) * y1)) % p()
    } else {
        let dx = (p() + y2 - y1) % p();
        let dy = (p() + x2 - x1) % p();
        dx * inv(&dy) % p()
    };
    let x3 = (p() + &lam * &lam - x1 - x2) % p();
    let y3 = (&lam * (p() + x1 - &x3) + p() - y1) % p();
    Some((x3, y3))
}

fn point_mul(mut k: BigUint, pt: &(BigUint, BigUint)) -> Option<(BigUint, BigUint)> {
    let mut result = None;
    let mut addend = Some(pt.clone());
    while k > BigUint::from(0u32) {
        if &k % 2u32 == BigUint::from(1u32) {
            result = point_add(result.as_ref(), addend.as_ref());
        }
        addend = point_add(addend.as_ref(), addend.as_ref());
        k >>= 1;
    }
    result
}

fn bytes32(value: &BigUint) -> [u8; 32] {
    let raw = value.to_bytes_be();
    let mut out = [0u8; 32];
    let n = raw.len().min(32);
    out[32 - n..].copy_from_slice(&raw[raw.len() - n..]);
    out
}

pub fn encode_public_key(q: &(BigUint, BigUint)) -> String {
    format!("04{}{}", hex::encode_upper(bytes32(&q.0)), hex::encode_upper(bytes32(&q.1)))
}

pub fn decode_public_key(pub_hex: &str) -> Result<(BigUint, BigUint), String> {
    let s = pub_hex.trim().replace(' ', "");
    let s = s.strip_prefix("04").unwrap_or(&s);
    if s.len() != 128 {
        return Err(format!("公钥长度不对: {}", s.len()));
    }
    Ok((hex_int(&s[..64]), hex_int(&s[64..])))
}

pub fn ecdh_shared_hex(d: &BigUint, peer: &(BigUint, BigUint)) -> Result<String, String> {
    let point = point_mul(d.clone(), peer).ok_or("ECDH 得到无穷远点")?;
    Ok(hex::encode_upper(bytes32(&point.0)))
}

pub fn derive_sm4_key(shared_hex: &str) -> String {
    let dig = sm3(shared_hex.as_bytes());
    hex::encode_upper(&dig[..16])
}

pub fn generate_keypair() -> (BigUint, (BigUint, BigUint)) {
    loop {
        let mut buf = [0u8; 32];
        getrandom::getrandom(&mut buf).expect("random");
        let d = BigUint::from_bytes_be(&buf) % n();
        if d == BigUint::from(0u32) {
            continue;
        }
        if let Some(q) = point_mul(d.clone(), g()) {
            return (d, q);
        }
    }
}

fn sm3(msg: &[u8]) -> [u8; 32] {
    const IV: [u32; 8] = [
        0x7380_166f, 0x4914_b2b9, 0x1724_42d7, 0xda8a_0600,
        0xa96f_30bc, 0x1631_38aa, 0xe38d_ee4d, 0xb0fb_0e4e,
    ];
    let mut data = msg.to_vec();
    let bits = (msg.len() as u64).saturating_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());
    let mut v = IV;
    for chunk in data.chunks(64) {
        let mut w = [0u32; 68];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for j in 16..68 {
            w[j] = p1(w[j - 16] ^ w[j - 9] ^ w[j - 3].rotate_left(15))
                ^ w[j - 13].rotate_left(7)
                ^ w[j - 6];
        }
        let mut w1 = [0u32; 64];
        for j in 0..64 {
            w1[j] = w[j] ^ w[j + 4];
        }
        let mut s = v;
        for j in 0..64 {
            let t: u32 = if j < 16 { 0x79cc_4519 } else { 0x7a87_9d8a };
            let ss1 = s[0]
                .rotate_left(12)
                .wrapping_add(s[4])
                .wrapping_add(t.rotate_left(j as u32))
                .rotate_left(7);
            let ss2 = ss1 ^ s[0].rotate_left(12);
            let tt1 = ff(j, s[0], s[1], s[2])
                .wrapping_add(s[3])
                .wrapping_add(ss2)
                .wrapping_add(w1[j]);
            let tt2 = gg(j, s[4], s[5], s[6])
                .wrapping_add(s[7])
                .wrapping_add(ss1)
                .wrapping_add(w[j]);
            s[3] = s[2];
            s[2] = s[1].rotate_left(9);
            s[1] = s[0];
            s[0] = tt1;
            s[7] = s[6];
            s[6] = s[5].rotate_left(19);
            s[5] = s[4];
            s[4] = p0(tt2);
        }
        for i in 0..8 {
            v[i] ^= s[i];
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in v.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn ff(j: usize, x: u32, y: u32, z: u32) -> u32 {
    if j < 16 { x ^ y ^ z } else { (x & y) | (x & z) | (y & z) }
}
fn gg(j: usize, x: u32, y: u32, z: u32) -> u32 {
    if j < 16 { x ^ y ^ z } else { (x & y) | (!x & z) }
}
fn p0(x: u32) -> u32 { x ^ x.rotate_left(9) ^ x.rotate_left(17) }
fn p1(x: u32) -> u32 { x ^ x.rotate_left(15) ^ x.rotate_left(23) }

const SBOX: [u8; 256] = [
    0xd6, 0x90, 0xe9, 0xfe, 0xcc, 0xe1, 0x3d, 0xb7, 0x16, 0xb6, 0x14, 0xc2, 0x28, 0xfb, 0x2c, 0x05,
    0x2b, 0x67, 0x9a, 0x76, 0x2a, 0xbe, 0x04, 0xc3, 0xaa, 0x44, 0x13, 0x26, 0x49, 0x86, 0x06, 0x99,
    0x9c, 0x42, 0x50, 0xf4, 0x91, 0xef, 0x98, 0x7a, 0x33, 0x54, 0x0b, 0x43, 0xed, 0xcf, 0xac, 0x62,
    0xe4, 0xb3, 0x1c, 0xa9, 0xc9, 0x08, 0xe8, 0x95, 0x80, 0xdf, 0x94, 0xfa, 0x75, 0x8f, 0x3f, 0xa6,
    0x47, 0x07, 0xa7, 0xfc, 0xf3, 0x73, 0x17, 0xba, 0x83, 0x59, 0x3c, 0x19, 0xe6, 0x85, 0x4f, 0xa8,
    0x68, 0x6b, 0x81, 0xb2, 0x71, 0x64, 0xda, 0x8b, 0xf8, 0xeb, 0x0f, 0x4b, 0x70, 0x56, 0x9d, 0x35,
    0x1e, 0x24, 0x0e, 0x5e, 0x63, 0x58, 0xd1, 0xa2, 0x25, 0x22, 0x7c, 0x3b, 0x01, 0x21, 0x78, 0x87,
    0xd4, 0x00, 0x46, 0x57, 0x9f, 0xd3, 0x27, 0x52, 0x4c, 0x36, 0x02, 0xe7, 0xa0, 0xc4, 0xc8, 0x9e,
    0xea, 0xbf, 0x8a, 0xd2, 0x40, 0xc7, 0x38, 0xb5, 0xa3, 0xf7, 0xf2, 0xce, 0xf9, 0x61, 0x15, 0xa1,
    0xe0, 0xae, 0x5d, 0xa4, 0x9b, 0x34, 0x1a, 0x55, 0xad, 0x93, 0x32, 0x30, 0xf5, 0x8c, 0xb1, 0xe3,
    0x1d, 0xf6, 0xe2, 0x2e, 0x82, 0x66, 0xca, 0x60, 0xc0, 0x29, 0x23, 0xab, 0x0d, 0x53, 0x4e, 0x6f,
    0xd5, 0xdb, 0x37, 0x45, 0xde, 0xfd, 0x8e, 0x2f, 0x03, 0xff, 0x6a, 0x72, 0x6d, 0x6c, 0x5b, 0x51,
    0x8d, 0x1b, 0xaf, 0x92, 0xbb, 0xdd, 0xbc, 0x7f, 0x11, 0xd9, 0x5c, 0x41, 0x1f, 0x10, 0x5a, 0xd8,
    0x0a, 0xc1, 0x31, 0x88, 0xa5, 0xcd, 0x7b, 0xbd, 0x2d, 0x74, 0xd0, 0x12, 0xb8, 0xe5, 0xb4, 0xb0,
    0x89, 0x69, 0x97, 0x4a, 0x0c, 0x96, 0x77, 0x7e, 0x65, 0xb9, 0xf1, 0x09, 0xc5, 0x6e, 0xc6, 0x84,
    0x18, 0xf0, 0x7d, 0xec, 0x3a, 0xdc, 0x4d, 0x20, 0x79, 0xee, 0x5f, 0x3e, 0xd7, 0xcb, 0x39, 0x48,
];

fn tau(x: u32) -> u32 {
    let b = x.to_be_bytes();
    u32::from_be_bytes([SBOX[b[0] as usize], SBOX[b[1] as usize], SBOX[b[2] as usize], SBOX[b[3] as usize]])
}
fn lin(x: u32) -> u32 { x ^ x.rotate_left(2) ^ x.rotate_left(10) ^ x.rotate_left(18) ^ x.rotate_left(24) }
fn lin_key(x: u32) -> u32 { x ^ x.rotate_left(13) ^ x.rotate_left(23) }

fn round_keys(key: &[u8; 16]) -> [u32; 32] {
    const FK: [u32; 4] = [0xa3b1_bac6, 0x56aa_3350, 0x677d_9197, 0xb270_22dc];
    let mut k = [0u32; 4];
    for i in 0..4 {
        k[i] = u32::from_be_bytes(key[i * 4..i * 4 + 4].try_into().unwrap()) ^ FK[i];
    }
    let mut rk = [0u32; 32];
    for i in 0..32 {
        let ck = u32::from_be_bytes([
            ((4 * i) * 7) as u8,
            ((4 * i + 1) * 7) as u8,
            ((4 * i + 2) * 7) as u8,
            ((4 * i + 3) * 7) as u8,
        ]);
        let t = k[1] ^ k[2] ^ k[3] ^ ck;
        let next = k[0] ^ lin_key(tau(t));
        rk[i] = next;
        k = [k[1], k[2], k[3], next];
    }
    rk
}

fn crypt_block(block: &[u8; 16], rk: &[u32; 32]) -> [u8; 16] {
    let mut x = [0u32; 4];
    for i in 0..4 {
        x[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for &k in rk {
        let t = x[1] ^ x[2] ^ x[3] ^ k;
        let next = x[0] ^ lin(tau(t));
        x = [x[1], x[2], x[3], next];
    }
    let mut out = [0u8; 16];
    for (i, word) in [x[3], x[2], x[1], x[0]].into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn pkcs7(data: &[u8]) -> Vec<u8> {
    let n = 16 - (data.len() % 16);
    let mut out = data.to_vec();
    out.extend(std::iter::repeat(n as u8).take(n));
    out
}

pub fn sm4_encrypt(key_hex: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
    let key = hex::decode(key_hex).map_err(|e| e.to_string())?;
    if key.len() != 16 {
        return Err("SM4 密钥必须是 16 字节".into());
    }
    let key: [u8; 16] = key.try_into().unwrap();
    let rk = round_keys(&key);
    let data = pkcs7(plain);
    let mut out = Vec::with_capacity(data.len());
    let mut prev = key;
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev[i];
        }
        let enc = crypt_block(&block, &rk);
        out.extend_from_slice(&enc);
        prev = enc;
    }
    Ok(out)
}

pub fn sm4_decrypt(key_hex: &str, cipher: &[u8]) -> Result<Vec<u8>, String> {
    let key = hex::decode(key_hex).map_err(|e| e.to_string())?;
    if key.len() != 16 || cipher.len() % 16 != 0 || cipher.is_empty() {
        return Err("SM4 密文长度不对".into());
    }
    let key: [u8; 16] = key.try_into().unwrap();
    let mut rk = round_keys(&key);
    rk.reverse();
    let mut out = Vec::with_capacity(cipher.len());
    let mut prev = key;
    for chunk in cipher.chunks(16) {
        let block: [u8; 16] = chunk.try_into().unwrap();
        let dec = crypt_block(&block, &rk);
        for i in 0..16 {
            out.push(dec[i] ^ prev[i]);
        }
        prev = block;
    }
    let n = *out.last().unwrap() as usize;
    if n == 0 || n > 16 || out.len() < n || out[out.len() - n..].iter().any(|b| *b as usize != n) {
        return Err("SM4 填充不对".into());
    }
    out.truncate(out.len() - n);
    Ok(out)
}

pub fn sm4_encrypt_hex(key_hex: &str, plain: &[u8]) -> Result<String, String> {
    Ok(hex::encode_upper(sm4_encrypt(key_hex, plain)?))
}

pub fn sm4_decrypt_hex(key_hex: &str, cipher_hex: &str) -> Result<Vec<u8>, String> {
    let raw = hex::decode(cipher_hex).map_err(|e| e.to_string())?;
    sm4_decrypt(key_hex, &raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm3_abc_and_sm4_block() {
        assert_eq!(hex::encode(sm3(b"abc")), "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0");
        let key = hex::decode("0123456789abcdeffedcba9876543210").unwrap();
        let pt = hex::decode("0123456789abcdeffedcba9876543210").unwrap();
        let rk = round_keys(key.as_slice().try_into().unwrap());
        let ct = crypt_block(pt.as_slice().try_into().unwrap(), &rk);
        assert_eq!(hex::encode(ct), "681edf34d206965e86b3e94f536e4246");
    }

    #[test]
    fn matches_python_xueqiu_vector() {
        let d = hex_int("1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF") % n();
        let q = point_mul(d.clone(), g()).unwrap();
        assert_eq!(
            encode_public_key(&q),
            "043CBB3D1177FFA1A21BB054AE613455EB409A77593D17221808977ECBFB0586479CF75393C8D257997F3FC1DF9E952449746FD75ED1307E4DD493AE3DD0A800BD"
        );
        let peer = point_mul(BigUint::from(2u32), g()).unwrap();
        let shared = ecdh_shared_hex(&d, &peer).unwrap();
        assert_eq!(shared, "8EBBE9B804CBA6EDC7FA09F938CCE07CAB3980885716F56FF2AB12BB610F4640");
        let key = derive_sm4_key(&shared);
        assert_eq!(key, "46D2B9341A0F76A38A2BE226690907FE");
        let ct = sm4_encrypt_hex(&key, b"client_id=JtXbaMn7eP&type=1").unwrap();
        assert_eq!(ct, "F2C3BF444D7757BD1CD312227C18D2A7EDA99261A11E9BF6B12E27420BD6FF32");
        assert_eq!(sm4_decrypt_hex(&key, &ct).unwrap(), b"client_id=JtXbaMn7eP&type=1");
    }
}
