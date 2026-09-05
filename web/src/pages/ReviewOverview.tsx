// 审核台的落地页（#377）：一个审核者进来时的三个问题——有多少在等、等了多久、
// 队列在消还是在涨。数据全来自 GET /review/summary，这里只画，不算：七档的数
// 与左栏是同一套口径（服务端共用 WHERE），页面上不再各自数一遍。
import type { ReviewSummary } from "../api";
import { S } from "../i18n";
import { Chip, LinkButton } from "../ui";

/** 总览里能点进去的七档 */
export type WaitingQueue = keyof ReviewSummary["waiting"];

/** 七档队列在总览里的顺序与左栏一致 */
const WAITING: { key: WaitingQueue; label: string }[] = [
  { key: "pending", label: S.review.railPending },
  { key: "duplicates", label: S.review.railDuplicates },
  { key: "conflicts", label: S.review.railConflicts },
  { key: "unconfirmed", label: S.review.railUnconfirmed },
  { key: "lowconf", label: S.review.railLowConfidence },
  { key: "violations", label: S.review.railViolations },
  { key: "defects", label: S.review.railDefects },
];

/** 从某个时刻到现在整几天；不满一天算 0 */
function daysSince(iso: string): number {
  return Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 86_400_000));
}

function SectionHead({ children }: { children: string }) {
  return (
    <h3 className="mb-3 text-fine font-medium uppercase tracking-[0.08em] text-ink-3">
      {children}
    </h3>
  );
}

/** 一格统计：大数在上，说明在下。卡片本身不是控件——想进那一档，点下面的
 *  句子；数字只是数字 */
function Stat({
  value,
  label,
  note,
  action,
}: {
  value: number | string;
  label: string;
  note?: string | null;
  action?: { label: string; onClick: () => void };
}) {
  return (
    <div className="glass flex flex-col gap-1 rounded-xl p-4">
      <div className="u-num text-display text-ink">{value}</div>
      <div className="text-body text-ink-2">{label}</div>
      {note && <div className="text-fine text-ink-3">{note}</div>}
      {action && (
        <div className="mt-2">
          <LinkButton onClick={action.onClick}>{action.label}</LinkButton>
        </div>
      )}
    </div>
  );
}

/** 近 14 天每天一根柱。**等距、含零**：服务端已把没有决定的天补成 0，这里
 *  不再插值。高度按这 14 天里的最大值归一，最矮留 2px 当基线，免得全零
 *  时什么都看不见 */
function DailyBars({ days }: { days: ReviewSummary["decided"]["daily"] }) {
  const max = Math.max(1, ...days.map((d) => d.count));
  return (
    <div>
      <div className="flex h-16 items-end gap-1">
        {days.map((d) => (
          <div
            key={d.day}
            className="flex-1 rounded-none bg-accent"
            style={{
              height: `${Math.max(2, Math.round((d.count / max) * 64))}px`,
              opacity: d.count === 0 ? 0.25 : 1,
            }}
            title={`${d.day} · ${d.count}`}
          />
        ))}
      </div>
      <div className="mt-1 flex justify-between text-fine text-ink-3">
        <span>{days[0]?.day}</span>
        <span>{days[days.length - 1]?.day}</span>
      </div>
    </div>
  );
}

export function ReviewOverview({
  summary,
  onPick,
}: {
  summary: ReviewSummary;
  /** 点某一档的「去处理」：落到左栏那一档 */
  onPick: (queue: WaitingQueue) => void;
}) {
  const { waiting, decided, health } = summary;
  const waitingTotal = WAITING.reduce((n, w) => n + waiting[w.key].count, 0);
  const share = (n: number) =>
    health.facts === 0 ? "—" : `${Math.round((n / health.facts) * 1000) / 10}%`;

  return (
    <div className="space-y-8">
      {/* 等着办的 */}
      <section>
        <SectionHead>{S.review.overviewWaiting}</SectionHead>
        {waitingTotal === 0 ? (
          <div className="glass rounded-xl p-8 text-center text-body text-ink-3">
            {S.review.overviewAllClear}
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-3 md:grid-cols-3 lg:grid-cols-4">
            {WAITING.filter((w) => waiting[w.key].count > 0).map((w) => {
              const q = waiting[w.key];
              return (
                <Stat
                  key={w.key}
                  value={q.count}
                  label={w.label}
                  note={q.oldest_at ? S.review.overviewOldest(daysSince(q.oldest_at)) : null}
                  action={{ label: S.review.overviewOpen, onClick: () => onPick(w.key) }}
                />
              );
            })}
          </div>
        )}
      </section>

      {/* 办过的 */}
      <section>
        <SectionHead>{S.review.overviewDecided}</SectionHead>
        <div className="grid grid-cols-2 gap-3">
          <Stat
            value={decided.last_7d.total}
            label={S.review.overviewLast7}
            note={S.review.overviewAutomatic(decided.last_7d.automatic)}
          />
          <Stat
            value={decided.last_30d.total}
            label={S.review.overviewLast30}
            note={S.review.overviewAutomatic(decided.last_30d.automatic)}
          />
        </div>
        <div className="glass mt-3 rounded-xl p-4">
          <div className="mb-3 text-fine text-ink-3">{S.review.overviewDaily}</div>
          <DailyBars days={decided.daily} />
        </div>
        {decided.last_30d.total === 0 ? (
          <p className="mt-3 text-small text-ink-3">{S.review.overviewNoDecisions}</p>
        ) : (
          <div className="mt-3 grid gap-3 md:grid-cols-2">
            <div className="glass rounded-xl p-4">
              <div className="mb-2 text-fine text-ink-3">{S.review.overviewByAction}</div>
              <div className="flex flex-wrap gap-2">
                {decided.last_30d.by_action.map((a) => (
                  <Chip key={a.action}>
                    {S.review.decisionActions[a.action] ?? a.action} · {a.count}
                  </Chip>
                ))}
              </div>
            </div>
            <div className="glass rounded-xl p-4">
              <div className="mb-2 text-fine text-ink-3">{S.review.overviewByActor}</div>
              <div className="space-y-1">
                {decided.last_30d.by_actor.map((a) => (
                  <div
                    key={a.actor_id ?? "auto"}
                    className="flex items-center justify-between text-body"
                  >
                    <span className="truncate text-ink-2">
                      {a.actor_id === null ? S.review.aiActor : (a.label ?? a.actor_id)}
                    </span>
                    <span className="u-num text-ink-3">{a.count}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </section>

      {/* 库的成色 */}
      <section>
        <SectionHead>{S.review.overviewHealth}</SectionHead>
        <p className="mb-3 text-small text-ink-3">{S.review.overviewFacts(health.facts)}</p>
        <div className="grid grid-cols-3 gap-3">
          <Stat
            value={health.low_confidence}
            label={S.review.railLowConfidence}
            note={share(health.low_confidence)}
          />
          <Stat
            value={health.unconfirmed}
            label={S.review.railUnconfirmed}
            note={share(health.unconfirmed)}
          />
          <Stat
            value={health.contested}
            label={S.review.overviewContested}
            note={share(health.contested)}
          />
        </div>
      </section>
    </div>
  );
}
