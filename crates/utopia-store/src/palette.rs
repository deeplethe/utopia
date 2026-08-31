//! 实体类的配色：一套莫兰迪色板，加一个**按 key 取色**的确定性函数。
//!
//! ## 为什么要有它
//!
//! 从前每个自动建出来的类都拿同一个 `#8ea5bd`——导入的、类型消解建的、
//! 建库时铺的，全是那一个灰蓝。产品里明明有一套精选色板，但它**只在人手动
//! 点开颜色选择器时才用得上**。于是：装一个 schema.org（1010 个类）进来，
//! 得到的是 1010 个类共用一种颜色，图上一片灰。
//!
//! 能力一直在，只是没接到自动那条路上。这个模块就是那截接线。
//!
//! ## 为什么是哈希而不是轮转
//!
//! 轮转（第 n 个类取第 n 个色）要维护一个计数器，而类是从好几条路建出来的
//! （手动、导入、消解、建库），计数器就得跨路共享——那是个会不同步的状态。
//! 按 key 哈希不需要状态：**同一个 key 永远同一个颜色**，无论它是谁建的、
//! 第几个建的、重建过几次。重导一遍本体，颜色不会跳。
//!
//! ## 为什么不用 `DefaultHasher`
//!
//! `std::collections::hash_map::DefaultHasher` 的种子**每个进程都不一样**
//! （防哈希碰撞攻击）。拿它取色，服务器一重启同一个类就换颜色——
//! 而颜色是用户用来认东西的。所以这里手写一个 FNV-1a：定死的常量，
//! 定死的结果，跨进程跨机器都一样。

/// 实体类的色板。**这一组是既有的，本次没有换**——换过一版莫兰迪，
/// 拿真实的图一看就否了：低饱和加中明度是为纸面调的，撞上近黑的画布会整片发灰，
/// 类与类之间分不开。深色底需要的是饱和度撑得住的颜色。
///
/// **改这里就得同步改 `web/src/ui/index.tsx` 的 `ENTITY_PALETTE`**：
/// 手动挑的色和自动取的色必须来自同一组，否则一张图里会出现两套配色。
/// 有测试盯着（见本文件末尾），改漏了会红。
pub const ENTITY_PALETTE: &[&str] = &[
    "#7fd0ff", "#5fa8ff", "#5fd4d0", "#63e2b7", "#4cc38a", "#a8d878", "#ffd479", "#f2b66d",
    "#ff9d76", "#ff8a9e", "#ff9daf", "#e797d8", "#c4a5ff", "#9fa8ff", "#8ea5bd", "#b3b9c4",
];

/// 类的形状：**方 = 词表声明的，圆 = 语料里长出来的**。
///
/// 从前所有自动建的类都写死 `circle`——跟颜色一样，能力在（画布有
/// `NodeSquareShellProgram`，图例也会跟着变方），只是没人给它值。
///
/// **为什么不像颜色那样哈希**：形状只有两个值，哈希出来是随机的——
/// `person` 是方是圆不代表任何事，那只是噪声。形状少而醒目，
/// 该承载一个真实区分。
///
/// **为什么是 IRI 而不是「顶层类」**：顶层类听着更自然，但很多知识库的类是
/// 扁平的（消解建出来的都没有父类），那样会变成「全是方的」，
/// 只是把问题反了个面。而有没有 IRI 是**确定的、当场就知道的**：
/// 导入的词表带 IRI，从语料里长出来的没有。
///
/// 这也正是这产品一直在讲的事——**说清楚一样东西是哪来的**。
/// 一张图上一眼就分得出「这是本体里声明过的类」和「这是从文档里长出来的类」。
pub fn shape_for(iri: &str) -> &'static str {
    if iri.trim().is_empty() {
        "circle"
    } else {
        "square"
    }
}

/// 类的 key → 颜色。同一个 key 永远得到同一个颜色。
///
/// FNV-1a，常量写死。别换成 `DefaultHasher`——那个每进程换种子，
/// 服务器一重启颜色就全变，而用户是靠颜色认东西的。
pub fn color_for_key(key: &str) -> &'static str {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
    for b in key.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    // **雪崩混合**：光 FNV 再取模会聚集——实测 person/project/research_lab
    // 撞进同一格，八个常见 key 只散到五种颜色。这一步把高位搅进低位，
    // 同样八个 key 就散开了。（色板长度不是质数，低位本身带不了多少信息）
    hash ^= hash >> 33;
    hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
    hash ^= hash >> 33;
    ENTITY_PALETTE[(hash % ENTITY_PALETTE.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **同一个 key 永远同一个颜色**——跨调用、跨进程。
    /// 这里把几个具体值钉死：换了哈希实现，已有知识库的颜色就会集体跳变。
    #[test]
    fn the_same_key_always_gets_the_same_colour() {
        for _ in 0..3 {
            assert_eq!(color_for_key("organization"), color_for_key("organization"));
        }
        // 钉住具体结果：不是为了这几个值本身好看，
        // 而是为了「换实现 = 已有库的配色全变」这件事必须是显式决定
        assert_eq!(color_for_key("person"), "#ff9d76");
        assert_eq!(color_for_key("organization"), "#e797d8");
        assert_eq!(color_for_key("product"), "#f2b66d");
    }

    /// 相邻的 key 不该撞成同一个色——否则一张图上「人」和「产品」分不开。
    /// 不保证全局无碰撞（十几个色装不下上千个类），但常见的几个要散开。
    #[test]
    fn the_common_keys_spread_across_the_palette() {
        let keys = [
            "person",
            "organization",
            "product",
            "event",
            "place",
            "document",
            "project",
            "team",
        ];
        let colours: std::collections::HashSet<_> = keys.iter().map(|k| color_for_key(k)).collect();
        assert!(
            colours.len() >= 6,
            "八个常见 key 只散到 {} 个色：{colours:?}",
            colours.len()
        );
    }

    /// 形状承载来历：词表声明的是方的，语料里长的是圆的。
    #[test]
    fn the_shape_says_where_the_class_came_from() {
        assert_eq!(shape_for("https://schema.org/Person"), "square");
        assert_eq!(shape_for(""), "circle");
        assert_eq!(shape_for("   "), "circle", "空白也算没有 IRI");
    }

    /// **前端的 colorForKey 必须与这里逐位一致。**
    ///
    /// 新建类时前端先按 key 挑一个颜色显示，用户不改就这么存下去；
    /// 而导入/消解那条路是后端算的。两边算得不一样，同一个 key 就会因为
    /// 「谁建的」拿到不同颜色——而这**不会有任何报错**。
    ///
    /// 这里只能检查前端那份代码在不在、结构对不对；数值一致性靠
    /// 「同一组常量 + 同一套运算」来保证，两边的注释互相指了路。
    /// 中文 key 尤其要当心：JS 那边必须按 UTF-8 字节遍历（TextEncoder），
    /// 按 char code 遍历算出来就不一样了。
    #[test]
    fn the_frontend_has_a_matching_hash() {
        let ts = include_str!("../../../web/src/ui/index.tsx");
        assert!(
            ts.contains("export function colorForKey"),
            "前端缺 colorForKey——新建类的颜色就会跟后端对不上"
        );
        // 这三样写错任何一个，算出来的颜色都会静默偏离
        assert!(ts.contains("0xcbf29ce484222325n"), "FNV 初值不对");
        assert!(ts.contains("0x100000001b3n"), "FNV 质数不对");
        assert!(ts.contains("0xff51afd7ed558ccdn"), "雪崩混合常量不对");
        assert!(
            ts.contains("TextEncoder"),
            "必须按 UTF-8 字节遍历：按 char code 走，中文 key 会算出别的颜色"
        );
    }

    /// **前后端的色板必须是同一组。**
    ///
    /// 手动挑色走前端的 `ENTITY_PALETTE`，自动取色走这里的。两边漂了，
    /// 一张图里就会出现两套配色，而且没有任何编译期检查会说话——
    /// 与 `sources::KINDS` 前后端不同步是同一类错（那次的症状是
    /// 界面上选得到、建的时候报 kind 不合法，只有端到端会撞上）。
    #[test]
    fn the_frontend_palette_matches_this_one() {
        let ts = include_str!("../../../web/src/ui/index.tsx");
        let start = ts
            .find("export const ENTITY_PALETTE")
            .expect("前端找不到 ENTITY_PALETTE");
        let body = &ts[start..start + ts[start..].find("];").expect("色板没有结尾") + 2];
        let front: Vec<&str> = body
            .lines()
            .filter_map(|l| {
                let t = l.trim().trim_end_matches(',').trim_matches('"');
                t.starts_with('#').then_some(t)
            })
            .collect();
        assert_eq!(
            front, ENTITY_PALETTE,
            "前后端色板不一致——改了一边就要改另一边"
        );
    }
}
