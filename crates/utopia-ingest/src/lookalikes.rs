//! PDF 文字层里长得一样、码位不同的字：部首与连字换回它们显示的那个字。
//!
//! PDF 画的是字形，文字层靠字体的 ToUnicode 表把字形指回码位。同一个字形在字体里常常
//! 既是一个部首又是一个统一汉字——苹方里「⼀」（U+2F00，康熙部首）和「一」（U+4E00）是
//! 同一个字形——ToUnicode 指到部首码位，读出来的字看着一样，码位却是部首：搜「日期」
//! 找不到「⽇期」，「王⼤⼒」和「王大力」是两个名字。macOS 生成的 PDF（Quartz、Chrome，
//! 用苹方）里，有部首孪生码位的汉字大都这样读出来。拉丁连字同理：「ﬁscal」的 ﬁ 是一个
//! 字形、一个码位（U+FB01）。
//!
//! 部首换成 Unicode 说它等同的那个统一汉字：UCD 的 `EquivalentUnifiedIdeograph.txt`，
//! 原样收在旁边。康熙部首 NFKC 也换得了，部首补充区（⻄、⻓）NFKC 不管，所以用这张表。
//! 表里的笔画不换：文档里写笔画，是当笔画用的。连字拆成字母。别的字一概不动——这不是
//! NFKC，全角标点、全角数字照旧。

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Unicode 18.0.0 的 EquivalentUnifiedIdeograph.txt，原样收录
const EQUIVALENTS: &str = include_str!("EquivalentUnifiedIdeograph.txt");

/// 两个部首区：部首补充（U+2E80–2EFF）与康熙部首（U+2F00–2FDF）
fn is_radical(c: char) -> bool {
    matches!(c, '\u{2E80}'..='\u{2EFF}' | '\u{2F00}'..='\u{2FDF}')
}

/// 部首 → 它等同的统一汉字。表里没有的部首（⺀）不在这里
fn radicals() -> &'static HashMap<char, char> {
    static MAP: OnceLock<HashMap<char, char>> = OnceLock::new();
    MAP.get_or_init(|| {
        let hex = |s: &str| u32::from_str_radix(s.trim(), 16).ok();
        let mut map = HashMap::new();
        // 一行一条：`2F08       ; 4EBA  #  KANGXI RADICAL MAN`，左边也可以是区间 `2E8C..2E8D`
        for line in EQUIVALENTS.lines() {
            let data = line.split('#').next().unwrap_or_default();
            let Some((from, to)) = data.split_once(';') else {
                continue;
            };
            let (lo, hi) = from.split_once("..").unwrap_or((from, from));
            let (Some(lo), Some(hi), Some(to)) =
                (hex(lo), hex(hi), hex(to).and_then(char::from_u32))
            else {
                continue;
            };
            for c in (lo..=hi)
                .filter_map(char::from_u32)
                .filter(|&c| is_radical(c))
            {
                map.insert(c, to);
            }
        }
        map
    })
}

/// 拉丁连字 → 字母（U+FB00–FB06，与 NFKC 拆得一样）
fn ligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

/// 部首、连字换回它们显示的那个字；一个都没有时原样借用
pub(crate) fn plain(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| is_radical(c) || ligature(c).is_some()) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match ligature(c) {
            Some(letters) => out.push_str(letters),
            None if is_radical(c) => out.push(radicals().get(&c).copied().unwrap_or(c)),
            None => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kangxi_radical_reads_as_the_ideograph_unicode_names() {
        let kangxi: Vec<char> = ('\u{2F00}'..='\u{2FD5}').collect();
        assert_eq!(kangxi.len(), 214);
        for c in kangxi {
            let ideograph = radicals().get(&c).copied();
            assert!(
                ideograph.is_some_and(|i| !is_radical(i)),
                "{c} (U+{:04X}) → {ideograph:?}",
                c as u32
            );
        }
        // 补充区也在表里，除了 Unicode 说没有等同字的 ⺀
        assert_eq!(radicals().len(), 214 + 114);
        assert_eq!(radicals().get(&'\u{2E80}'), None);
    }

    #[test]
    fn the_lookalikes_of_a_mac_pdf_read_as_the_characters_they_show() {
        let read = "项⽬负责⼈为王⼤⼒，现⾦流量净额同⽐增⻓。⼀⾏⻄⺟⻳⺅ ﬁscal ﬂow ﬄ";
        assert_eq!(
            plain(read),
            "项目负责人为王大力，现金流量净额同比增长。一行西母龟亻 fiscal flow ffl"
        );
        // 没有等同字的部首照旧
        assert_eq!(plain("⺀"), "⺀");
    }

    #[test]
    fn other_text_is_borrowed_as_it_is() {
        // 全角标点、全角数字、全角字母不动：这不是 NFKC
        let text = "营业收入１２５，４３０元（同比增长１３．７７％）；ＡＢＣ “引号” café";
        assert!(matches!(plain(text), Cow::Borrowed(t) if t == text));
    }
}
