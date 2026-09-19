/** 世界轴日期的读与写，一条规则，全站共用。
 *
 *  服务端的 `time_text`（给模型看的）与导出的 `rdf::world_time`（给审计者的）是同一条
 *  规则的另两份；三处分叉，人看到的、模型看到的、审计者拿到的就不是同一个日期。 */

/** 世界轴的一端按**自己的精度**显示：year → 2023，month → 2023-06，day → 2023-06-01。
 *  多少精度显示多少位——从前一律到日，年精度的事实显示成 1 月 1 日，是在无知的
 *  地方填一个确定的值。没有精度的日期（派生行上来自证据的界，0022）按日显示。
 *
 *  按 UTC 取年月日而不是本地：`valid_from` / `valid_to` 来自文档里的陈述
 * （"2019 年 5 月 2 日就任"），是**日历日期不是时刻**，本来就没有时区；存的是那一天
 *  的 UTC 午夜。按本地渲染会让 UTC-5 的读者看到 2019-05-01——凭空差一天，而且差的
 *  方向还随读者所在地变。记录时间（我们何时这么认为）是另一回事，那个该按本地，
 *  见 EntityHistory 里 ymd 的注释。 */
export function fmtTime(
  iso: string | null,
  precision: string | null,
): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  const y = d.getUTCFullYear();
  const m = String(d.getUTCMonth() + 1).padStart(2, "0");
  const day = String(d.getUTCDate()).padStart(2, "0");
  if (precision === "year") return `${y}`;
  if (precision === "month") return `${y}-${m}`;
  // 小时以下（0024）：ISO 的缩略形式，带 Z——钟点没有时区就不是时刻
  const hh = String(d.getUTCHours()).padStart(2, "0");
  const mi = String(d.getUTCMinutes()).padStart(2, "0");
  const ss = String(d.getUTCSeconds()).padStart(2, "0");
  if (precision === "hour") return `${y}-${m}-${day}T${hh}Z`;
  if (precision === "minute") return `${y}-${m}-${day}T${hh}:${mi}Z`;
  if (precision === "second") return `${y}-${m}-${day}T${hh}:${mi}:${ss}Z`;
  return `${y}-${m}-${day}`;
}

/** 输入框里的日期 → 时间戳 + 精度。**写多少位就是多少精度**，与抽取端
 *  `parse_time` 同一个约定：2023 是「那一年」，2023-06 是「那个月」。
 *
 *  这也是不用日期选择器的理由——选择器逼人给出一个日，而「文档只说了上半年」
 *  正是这个表单要修的那种错。回读比对是因为 `new Date("2023-13")` 不报错。 */
export function parseDateInput(
  s: string,
): { iso: string; precision: string } | null {
  const t = s.trim();
  const shape =
    /^(\d{4})(?:-(\d{2}))?(?:-(\d{2}))?(?:T(\d{2})(?::(\d{2}))?(?::(\d{2}))?(Z|[+-]\d{2}:\d{2})?)?$/.exec(
      t,
    );
  if (!shape) return null;
  const [, y, m, d, hh, mi, ss, zone] = shape;
  const iso = `${y}-${m ?? "01"}-${d ?? "01"}T00:00:00.000Z`;
  const calendar = new Date(iso);
  if (Number.isNaN(calendar.getTime())) return null;
  // Validate the written calendar date before applying its offset: Date silently
  // rolls February 30 into March, while a valid offset can legitimately change the day.
  if (calendar.toISOString().slice(0, 10) !== iso.slice(0, 10)) return null;
  if (hh === undefined) {
    return { iso, precision: d ? "day" : m ? "month" : "year" };
  }
  // 钟点（0024）：没有时区就不是一个时刻——不猜，让人补上 Z 或 +08:00；
  // 有了时区换成 UTC，值截到写出来的那一位
  if (!d || !zone) return null;
  const parsed = new Date(`${y}-${m}-${d}T${hh}:${mi ?? "00"}:${ss ?? "00"}${zone}`);
  if (Number.isNaN(parsed.getTime())) return null;
  return {
    iso: parsed.toISOString(),
    precision: ss !== undefined ? "second" : mi !== undefined ? "minute" : "hour",
  };
}
