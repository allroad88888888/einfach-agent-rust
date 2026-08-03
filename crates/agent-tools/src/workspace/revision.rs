//! 文件内容 revision。
//!
//! revision 是供工具调用方保存并在变更前回传的稳定、不透明 token。它只描述
//! 文件内容；文件类型、路径代次和持久化 journal 代次由后续工作包补充到调用层。

use std::fmt;

/// 由文件内容确定的、不透明 revision token。
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Revision(String);

impl Revision {
    /// 不存在目标的稳定前置条件 token。
    pub(crate) fn absent() -> Self {
        Self("absent:v1".to_owned())
    }

    /// 计算 `contents` 的 SHA-256 revision。
    pub(crate) fn for_contents(contents: &[u8]) -> Self {
        let digest = sha256(contents);
        let mut token = String::with_capacity("file:sha256:v1:".len() + 64);
        token.push_str("file:sha256:v1:");
        for byte in digest {
            use fmt::Write as _;
            write!(&mut token, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(token)
    }

    /// 比较调用方提供的前置条件与当前内容状态。
    pub(crate) fn matches(&self, current: &Self) -> bool {
        self == current
    }

    /// 为工具 JSON 输出提供 token 字符串。
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// 从持久化 manifest 读取既有 token。
    pub(crate) fn from_token(token: &str) -> Result<Self, crate::ToolError> {
        Self::parse(
            token,
            "journal_needs_repair",
            "workspace journal revision 非法",
        )
    }

    /// 从工具调用的乐观并发前置条件读取 token。
    pub(crate) fn from_expected_token(token: &str) -> Result<Self, crate::ToolError> {
        Self::parse(token, "bad_input", "expected_revision 格式非法")
    }

    fn parse(
        token: &str,
        error_code: &'static str,
        error_message: &'static str,
    ) -> Result<Self, crate::ToolError> {
        let valid_absent = token == "absent:v1";
        let valid_hash = token.len() == "file:sha256:v1:".len() + 64
            && token.starts_with("file:sha256:v1:")
            && token["file:sha256:v1:".len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit());
        if valid_absent || valid_hash {
            Ok(Self(token.to_owned()))
        } else {
            Err(crate::ToolError {
                code: error_code.into(),
                message: error_message.into(),
            })
        }
    }
}

impl fmt::Debug for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Revision").field(&self.0).finish()
    }
}

/// SHA-256 的单块压缩常量。
const ROUND_CONSTANTS: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

fn sha256(input: &[u8]) -> [u8; 32] {
    let mut state = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];

    let full_blocks = input.len() / 64;
    for block in input[..full_blocks * 64].chunks_exact(64) {
        compress(&mut state, block);
    }

    let mut final_blocks = [0_u8; 128];
    let tail = &input[full_blocks * 64..];
    final_blocks[..tail.len()].copy_from_slice(tail);
    final_blocks[tail.len()] = 0x80;
    let final_len = if tail.len() < 56 { 64 } else { 128 };
    let bit_len = (input.len() as u64).wrapping_mul(8).to_be_bytes();
    final_blocks[final_len - 8..final_len].copy_from_slice(&bit_len);
    for block in final_blocks[..final_len].chunks_exact(64) {
        compress(&mut state, block);
    }

    let mut digest = [0_u8; 32];
    for (index, word) in state.into_iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut schedule = [0_u32; 64];
    for (index, word) in schedule[..16].iter_mut().enumerate() {
        let start = index * 4;
        *word = u32::from_be_bytes(
            block[start..start + 4]
                .try_into()
                .expect("block is 64 bytes"),
        );
    }
    for index in 16..64 {
        let small_sigma_0 = schedule[index - 15].rotate_right(7)
            ^ schedule[index - 15].rotate_right(18)
            ^ (schedule[index - 15] >> 3);
        let small_sigma_1 = schedule[index - 2].rotate_right(17)
            ^ schedule[index - 2].rotate_right(19)
            ^ (schedule[index - 2] >> 10);
        schedule[index] = schedule[index - 16]
            .wrapping_add(small_sigma_0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(small_sigma_1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for (constant, word) in ROUND_CONSTANTS.into_iter().zip(schedule) {
        let big_sigma_1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ ((!e) & g);
        let temporary_1 = h
            .wrapping_add(big_sigma_1)
            .wrapping_add(choice)
            .wrapping_add(constant)
            .wrapping_add(word);
        let big_sigma_0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temporary_2 = big_sigma_0.wrapping_add(majority);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temporary_1);
        d = c;
        c = b;
        b = a;
        a = temporary_1.wrapping_add(temporary_2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

#[cfg(test)]
mod tests {
    use super::Revision;

    #[test]
    fn contents_use_a_stable_sha256_token() {
        let revision = Revision::for_contents(b"abc");
        assert_eq!(
            revision.as_str(),
            "file:sha256:v1:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn contents_hashes_empty_and_two_block_standard_vectors() {
        assert_eq!(
            Revision::for_contents(b"").as_str(),
            "file:sha256:v1:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Revision::for_contents(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
                .as_str(),
            "file:sha256:v1:248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn revisions_compare_only_when_contents_match() {
        let expected = Revision::for_contents(b"before");
        assert!(expected.matches(&Revision::for_contents(b"before")));
        assert!(!expected.matches(&Revision::for_contents(b"after")));
        assert!(!expected.matches(&Revision::absent()));
    }

    #[test]
    fn absent_is_distinct_from_an_empty_file() {
        assert_ne!(Revision::absent(), Revision::for_contents(b""));
    }
}
