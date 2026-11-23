//! 加密與 topic 名稱。
//!
//! broker 是公開的, 任何人都能訂閱 `#`, 所以:
//! - 群組與機器在 topic 裡都用雜湊值, 看不出是誰
//! - 內容用共用金鑰(cluster.key)做 ChaCha20-Poly1305 加密與驗證,
//!   並把 topic 當成附加資料(AAD), 訊息不能被搬到別的 topic 重放

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};

/// 訊息格式版本(第一個 byte)
const VERSION: u8 = 1;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    for p in parts {
        // 每段加長度, 避免 "ab"+"c" 和 "a"+"bc" 相同
        buf.extend_from_slice(&(p.len() as u32).to_be_bytes());
        buf.extend_from_slice(p);
    }
    let d = digest(&SHA256, &buf);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_ref());
    out
}

pub struct Cipher {
    key: LessSafeKey,
    /// 用來推導 topic 的祕密(和加密金鑰分開)
    topic_secret: [u8; 32],
    rng: SystemRandom,
}

impl Cipher {
    pub fn new(secret: &str) -> Self {
        let enc = sha256(&[b"cng6-cluster-enc-v1", secret.as_bytes()]);
        let topic_secret = sha256(&[b"cng6-cluster-topic-v1", secret.as_bytes()]);
        Self {
            key: LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &enc).expect("32 bytes")),
            topic_secret,
            rng: SystemRandom::new(),
        }
    }

    /// 群組的 topic 前綴: 同一個 server 和同一把金鑰才會相同
    pub fn group_prefix(&self, server: &str) -> String {
        let h = sha256(&[&self.topic_secret, b"group", server.as_bytes()]);
        format!("cng6/{}", &hex(&h)[..24])
    }

    /// 一台機器的 status topic
    pub fn status_topic(&self, server: &str, name: &str) -> String {
        let h = sha256(&[&self.topic_secret, b"node", name.as_bytes()]);
        format!("{}/{}/status", self.group_prefix(server), &hex(&h)[..16])
    }

    /// 訂閱群組裡所有機器的 status
    pub fn status_filter(&self, server: &str) -> String {
        format!("{}/+/status", self.group_prefix(server))
    }

    /// 加密: `版本 | nonce | 密文+tag`
    pub fn seal(&self, topic: &str, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        self.rng.fill(&mut nonce).expect("random");
        let mut buf = plain.to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(topic.as_bytes()),
                &mut buf,
            )
            .expect("seal");
        let mut out = Vec::with_capacity(1 + NONCE_LEN + buf.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&buf);
        out
    }

    /// 解密並驗證; 金鑰不同或被竄改時回傳 None
    pub fn open(&self, topic: &str, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 1 + NONCE_LEN || data[0] != VERSION {
            return None;
        }
        let nonce = Nonce::try_assume_unique_for_key(&data[1..1 + NONCE_LEN]).ok()?;
        let mut buf = data[1 + NONCE_LEN..].to_vec();
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(topic.as_bytes()), &mut buf)
            .ok()?;
        Some(plain.to_vec())
    }

    /// 隨機的十六進位字串(連線 id、session)
    pub fn random_hex(&self, bytes: usize) -> String {
        let mut b = vec![0u8; bytes];
        self.rng.fill(&mut b).expect("random");
        hex(&b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open() {
        let c = Cipher::new("祕密");
        let t = c.status_topic("pi5", "pi4");
        let sealed = c.seal(&t, b"hello");
        assert_eq!(c.open(&t, &sealed).unwrap(), b"hello");
        // 換 topic、換金鑰、被竄改都打不開
        assert!(c.open("other/topic", &sealed).is_none());
        assert!(Cipher::new("別的").open(&t, &sealed).is_none());
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(c.open(&t, &bad).is_none());
        assert!(c.open(&t, b"x").is_none());
        // 每次加密結果不同(nonce 隨機)
        assert_ne!(c.seal(&t, b"hello"), sealed);
    }

    #[test]
    fn topics() {
        let a = Cipher::new("k");
        let b = Cipher::new("k2");
        // 同金鑰同 server → 同群組; 名稱不會出現在 topic 裡
        assert_eq!(a.group_prefix("pi5"), Cipher::new("k").group_prefix("pi5"));
        assert_ne!(a.group_prefix("pi5"), b.group_prefix("pi5"));
        assert_ne!(a.group_prefix("pi5"), a.group_prefix("pi4"));
        let t = a.status_topic("pi5", "laptop");
        assert!(t.starts_with(&a.group_prefix("pi5")));
        assert!(!t.contains("laptop") && !t.contains("pi5"));
        assert!(t.ends_with("/status"));
        assert_eq!(a.status_filter("pi5"), format!("{}/+/status", a.group_prefix("pi5")));
    }
}
