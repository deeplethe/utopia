/** 字面值宾语的显示：属性 {value,unit} / 问数映射 {summary} / 其他 JSON 兜底。
 *
 *  一处实现，三个页面用（图谱面板、待点头列表、历史）。从前各写各的，
 *  单位的贴法就分了叉。
 *
 *  **值里已经带着单位的，不再贴一遍。**未采纳的字面值照原文落笔（`"75.0%"`、
 *  `"$2.22"`），单位另记一格是为了采纳时换算；显示时若不看值本身，就成了
 *  `75.0%%` 和 `$2.22 $`。采纳过的属性值是数（`75`、`5e9`），单位才是要贴的。 */
export function fmtObjectValue(v: Record<string, unknown> | null): string | null {
  if (!v) return null;
  if (v.value !== undefined) {
    if (typeof v.value === "boolean") return v.value ? "✓" : "✗";
    const unit = typeof v.unit === "string" && v.unit.trim() ? v.unit.trim() : "";
    /* 大数收成 `$5B`。金额存进来是**乘开的数**（`$5 billion` → 5000000000），
       因为值要能比大小才有资格不当节点；可原样念出来是「5000000000 $」，
       比原文那句「$5 billion」难读得多。符号在前、数收成紧凑写法，两头都要 */
    if (typeof v.value === "number" && unit && unit !== "%") {
      /* 两位小数只给紧凑写法：值是按声明单位读出来的，「350万元」进单位亿元的属性是
         0.035，舍到两位就显示成 0.04，「3万元」成了 0。普通写法留十五位有效数，
         也不把规则算术的 0.1 + 0.2 显示成 0.30000000000000004 */
      const compact = Math.abs(v.value) >= 10000;
      const n = new Intl.NumberFormat(undefined, {
        notation: compact ? "compact" : "standard",
        ...(compact ? { maximumFractionDigits: 2 } : { maximumSignificantDigits: 15 }),
      }).format(v.value);
      // Currency symbols precede an amount; physical units follow a quantity.
      return /\p{Sc}/u.test(unit) ? `${unit}${n}` : `${n} ${unit}`;
    }
    const val = String(v.value);
    if (!unit) return val;
    if (carriesUnit(val, unit)) return val;
    // 百分号紧贴着数，别的单位空一格
    return unit === "%" ? `${val}%` : `${val} ${unit}`;
  }
  if (typeof v.summary === "string") return v.summary;
  return JSON.stringify(v);
}

/** 原文写法里单位已经在了：`75.0%`、`$2.22`、`€30 million`、`500 MW` */
function carriesUnit(val: string, unit: string): boolean {
  const t = val.trim();
  return t.startsWith(unit) || t.endsWith(unit) || t.includes(` ${unit} `) || t.includes(`${unit} `);
}
