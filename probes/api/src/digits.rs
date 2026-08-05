//! 把一串数字画成 8-bit 灰度位图（白底黑字）。
//!
//! 存在理由只有一个：探针要回答的不是「API 收不收这个请求」，而是「模型**真的
//! 看见了没有**」——200 不等于看见（E9 在消息级 system 上踩过同一个坑：收 ≠ 听）。
//! 只有把一个它没处猜的东西印进像素里，这条才判定得了。
//!
//! 所以要能把一个随机四位数变成图。字库只有 0-9 十个 5×7 字形，不引字体依赖。

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
/// 字与字之间空这么多字形像素。
const GLYPH_GAP: usize = 1;

/// 0-9 的 5×7 字形，每行一个 5 位掩码，最高位（bit 4）是最左边那列，1 = 黑。
const FONT: [[u8; GLYPH_H]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// 行优先，一字节一像素。
    pub pixels: Vec<u8>,
}

/// 把 `text` 里的数字字符渲染成位图；非数字字符直接跳过。
///
/// `scale` 是一个字形像素放大成多少个真实像素，`margin` 是四周留白（真实像素）。
pub fn render(text: &str, scale: usize, margin: usize) -> Bitmap {
    let glyphs: Vec<usize> = text.chars().filter_map(|c| c.to_digit(10)).map(|d| d as usize).collect();
    assert!(!glyphs.is_empty(), "没有可渲染的数字");

    let cells_w = glyphs.len() * GLYPH_W + (glyphs.len() - 1) * GLYPH_GAP;
    let width = cells_w * scale + margin * 2;
    let height = GLYPH_H * scale + margin * 2;
    let mut pixels = vec![255u8; width * height];

    for (i, &d) in glyphs.iter().enumerate() {
        let x0 = margin + i * (GLYPH_W + GLYPH_GAP) * scale;
        for (row, mask) in FONT[d].iter().enumerate() {
            for col in 0..GLYPH_W {
                // bit 4 是最左列。
                if mask >> (GLYPH_W - 1 - col) & 1 == 0 {
                    continue;
                }
                blacken(&mut pixels, width, x0 + col * scale, margin + row * scale, scale);
            }
        }
    }

    Bitmap { width: width as u32, height: height as u32, pixels }
}

/// 把一个字形像素放大后的方块涂黑。
fn blacken(pixels: &mut [u8], width: usize, x: usize, y: usize, scale: usize) {
    for dy in 0..scale {
        let row = (y + dy) * width;
        pixels[row + x..row + x + scale].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_accounts_for_gaps_and_margin() {
        let bmp = render("1234", 10, 20);
        // 4 字 × 5 列 + 3 个间隔 = 23 个字形像素
        assert_eq!(bmp.width, 23 * 10 + 40);
        assert_eq!(bmp.height, 7 * 10 + 40);
        assert_eq!(bmp.pixels.len(), (bmp.width * bmp.height) as usize);
    }

    /// 白底黑字：既得有黑的（不然是空白图），也得有白的（不然全黑）。
    /// 这条挡的是「掩码 bit 序搞反」——搞反了图还是有黑有白，但认不出是几，
    /// 所以下一条按字形逐位对。
    #[test]
    fn renders_both_ink_and_paper() {
        let bmp = render("7", 3, 1);
        assert!(bmp.pixels.contains(&0), "全白 = 什么都没画");
        assert!(bmp.pixels.contains(&255), "全黑 = 涂错了");
    }

    /// scale=1、margin=0 时，位图应当逐像素等于字形本身。掩码取位方向错了
    /// 这里立刻镜像，而真机上只会表现为「模型认错了数字」——那时分不清是
    /// 模型不行还是我画反了。
    #[test]
    fn scale_one_reproduces_the_glyph_exactly() {
        let bmp = render("2", 1, 0);
        let expect: Vec<u8> = FONT[2]
            .iter()
            .flat_map(|mask| (0..GLYPH_W).map(move |c| if mask >> (GLYPH_W - 1 - c) & 1 == 1 { 0 } else { 255 }))
            .collect();
        assert_eq!(bmp.pixels, expect);
    }
}
