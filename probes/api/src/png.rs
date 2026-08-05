//! 零依赖的最小 PNG 编码器：8-bit 灰度位图 → PNG 字节。
//!
//! 为什么手写而不是加一条 `png` 依赖：探针素材必须**逐字节确定**（见 `fixture`
//! 模块开头那句——素材里混进任何不确定的东西，前缀缓存就没有可比性了）。压缩库的
//! 输出跨版本不保证一致，而这里根本不需要压缩：deflate 的 **stored（不压缩）块**
//! 五个字节一个头就够用，图片再大也只是多几个块。顺带还省掉一条依赖。
//!
//! 只支持 color type 0（灰度）/ bit depth 8 / 不隔行——画一个数字够了。

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// stored 块的长度字段是 u16，所以一块最多这么多字节。
const MAX_STORED: usize = 0xFFFF;

/// `pixels` 按行优先，一个字节一个像素（0 = 黑，255 = 白）。
pub fn encode_gray(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(
        pixels.len(),
        width as usize * height as usize,
        "像素数与宽高不符：{}x{} 要 {} 个，给了 {}",
        width,
        height,
        width as usize * height as usize,
        pixels.len()
    );

    let mut out = Vec::from(SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    // 位深 8 / 灰度 / deflate / 标准滤波 / 不隔行
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &zlib_stored(&scanlines(width, height, pixels)));
    push_chunk(&mut out, b"IEND", &[]);
    out
}

/// 每行前面加一个滤波类型字节 0（None）——不做行间预测，配 stored 块最直白。
fn scanlines(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let w = width as usize;
    let mut raw = Vec::with_capacity((w + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0);
        raw.extend_from_slice(&pixels[y * w..(y + 1) * w]);
    }
    raw
}

/// zlib 容器 + 全 stored 的 deflate 流。
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    // 0x78 0x01：CM=8、CINFO=7、FLEVEL=0，且 0x7801 % 31 == 0（zlib 头的校验约束）。
    let mut out = vec![0x78, 0x01];
    // 空输入也得有一个块，否则不是合法 deflate 流。
    if raw.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    let mut chunks = raw.chunks(MAX_STORED).peekable();
    while let Some(part) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(u8::from(last)); // BFINAL + BTYPE=00（stored）
        let len = part.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    // CRC 覆盖 type + data，不含长度字段。
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 头部字节是死的，写错了整张图打不开——但打不开这件事要等到真机才暴露，
    /// 太晚。这里钉住签名与 IHDR 的字面值。
    #[test]
    fn header_bytes_are_exact() {
        let png = encode_gray(2, 1, &[0, 255]);
        assert_eq!(&png[..8], &SIGNATURE);
        assert_eq!(&png[8..12], &13u32.to_be_bytes()); // IHDR 的数据长度固定 13
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[20..24], &1u32.to_be_bytes());
        assert_eq!(&png[24..29], &[8, 0, 0, 0, 0]);
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    /// CRC32 的标准测试向量。自己手写的表驱动算法一旦搞反 bit 序，
    /// 生成的 PNG 会被所有解码器拒绝，而错误信息只会说「文件损坏」。
    #[test]
    fn crc32_matches_the_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_matches_the_known_vector() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
