// 审核台的落地页（#377）：一个审核者进来时的三个问题——有多少在等、等了多久、
// 队列在消还是在涨。数据全来自 GET /review/summary，这里只画，不算：七档的数
// 与左栏是同一套口径（服务端共用 WHERE），页面上不再各自数一遍。
import type { ReviewSummary } from "../api";
import { S } from "../i18n";
import { Chip, GroupLabel, LinkButton } from "../ui";

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
  { key: "alignment", label: S.review.railAlignment },
];

/** 从某个时刻到现在整几天；不满一天算 0 */
function daysSince(iso: string): number {
  return Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 86_400_000));
}

function SectionHead({ children }: { children: string }) {
  return <GroupLabel className="mb-3">{children}</GroupLabel>;
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
    <div className="glass flex flex-col gap-1 rounded-panel p-4">
      <div className="u-num text-display text-ink">{value}</div>
      <div className="text-body text-ink-2">{label}</div>
      {note && <div className="text-fine text-ink-2">{note}</div>}
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
      {/* 一天一格、格里居中一根柱，**柱子本身有上限宽**。从前是 `flex-1`
          直接铺满：这张卡横着能有一千多像素，十四根柱子就成了十四块板，
          有数的那天看着像一堵墙而不是一根柱 */}
      <div className="flex h-16 items-end gap-1">
        {days.map((d) => (
          <div key={d.day} className="flex h-full flex-1 items-end justify-center">
            <div
              className="w-full max-w-[18px] rounded-none bg-bar"
              style={{
                height: `${Math.max(2, Math.round((d.count / max) * 64))}px`,
                opacity: d.count === 0 ? 0.25 : 1,
              }}
              title={`${d.day} · ${d.count}`}
            />
          </div>
        ))}
      </div>
      <div className="mt-1 flex justify-between text-fine text-ink-2">
        <span>{days[0]?.day}</span>
        <span>{days[days.length - 1]?.day}</span>
      </div>
    </div>
  );
}

export function ReviewOverview({
  summary,
  governance,
  onPick,
  onSettings,
}: {
  summary: ReviewSummary;
  /** 这个库的治理开关（0025）：关着时 Agent 那一段说明去哪里打开 */
  governance: boolean;
  /** 点某一档的「去处理」：落到左栏那一档 */
  onPick: (queue: WaitingQueue | "agent") => void;
  onSettings: () => void;
}) {
  const { waiting, decided, health, agent } = summary;
  const waitingTotal = WAITING.reduce((n, w) => n + waiting[w.key].count, 0);
  const share = (n: number) =>
    health.facts === 0 ? "—" : `${Math.round((n / health.facts) * 1000) / 10}%`;

  return (
    <div className="space-y-8">
      {/* 等着办的 */}
      <section>
        <SectionHead>{S.review.overviewWaiting}</SectionHead>
        {waitingTotal === 0 ? (
          <div className="glass rounded-panel p-8 text-center text-body text-ink-2">
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
        <div className="glass mt-3 rounded-panel p-4">
          <div className="mb-3 text-fine text-ink-2">{S.review.overviewDaily}</div>
          <DailyBars days={decided.daily} />
        </div>
        {decided.last_30d.total === 0 ? (
          <p className="mt-3 text-small text-ink-2">{S.review.overviewNoDecisions}</p>
        ) : (
          <div className="mt-3 grid gap-3 md:grid-cols-2">
            <div className="glass rounded-panel p-4">
              <div className="mb-2 text-fine text-ink-2">{S.review.overviewByAction}</div>
              <div className="flex flex-wrap gap-2">
                {decided.last_30d.by_action.map((a) => (
                  <Chip key={a.action}>
                    {S.review.decisionActions[a.action] ?? a.action} · {a.count}
                  </Chip>
                ))}
              </div>
            </div>
            <div className="glass rounded-panel p-4">
              <div className="mb-2 text-fine text-ink-2">{S.review.overviewByActor}</div>
              <div className="space-y-1">
                {decided.last_30d.by_actor.map((a) => (
                  <div
                    key={a.actor_id ?? "auto"}
                    className="flex items-center justify-between text-body"
                  >
                    <span className="truncate text-ink-2">
                      {a.actor_id === null ? S.review.aiActor : (a.label ?? a.actor_id)}
                    </span>
                    <span className="u-num text-ink-2">{a.count}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
        )}
      </section>

      {/* agent 做过的（0025）：开着的建议、自动裁了还站着的、人接受 / 改判 / 撤回的。
          开关关着且什么都没做过时只说一句去哪里打开，不摆四个零 */}
      <section>
        <SectionHead>{S.review.overviewAgent}</SectionHead>
        {!governance && (
          <p className="mb-3 text-small text-ink-2">
            {S.review.overviewAgentOff}{" "}
            <LinkButton onClick={onSettings}>{S.review.overviewAgentSettings}</LinkButton>
          </p>
        )}
        {/* 此刻在跑：一颗脉动的点加还剩几对；没跑但有积压：几对等它 */}
        {agent.running ? (
          <p className="mb-3 flex items-center text-small text-ink-2">
            <span className="mr-2 inline-block h-2 w-2 rounded-full bg-warn animate-pulse" />
            {S.review.overviewAgentRunning(agent.queue)}
          </p>
        ) : governance && agent.queue > 0 ? (
          <p className="mb-3 text-small text-ink-2">{S.review.overviewAgentQueue(agent.queue)}</p>
        ) : null}
        {(governance || agent.open > 0 || agent.last_30d.applied > 0) && (
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            <Stat
              value={agent.open}
              label={S.review.overviewAgentOpen}
              action={
                agent.open > 0
                  ? { label: S.review.overviewOpen, onClick: () => onPick("agent") }
                  : undefined
              }
            />
            <Stat
              value={agent.last_7d.applied}
              label={S.review.overviewAgentApplied}
              note={S.review.overviewLast7}
            />
            <Stat
              value={agent.last_30d.accepted}
              label={S.review.overviewAgentAccepted}
              note={S.review.overviewAgentOverridden(agent.last_30d.overridden)}
            />
            <Stat
              value={agent.last_30d.reverted}
              label={S.review.overviewAgentReverted}
              note={S.review.overviewLast30}
            />
          </div>
        )}
      </section>

      {/* 库的成色 */}
      <section>
        <SectionHead>{S.review.overviewHealth}</SectionHead>
        <p className="mb-3 text-small text-ink-2">{S.review.overviewFacts(health.facts)}</p>
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
