//! 一整个用户轮次共用的一个历史窗口和一次上下文超限恢复（#964）。
//!
//! 能丢的只有此前已完成或已失败的整段问答，以及上一轮的工具往返；本轮的问题、系统
//! 提示和本轮已经拿到的工具结果一个字都不动，工具也不重跑。落库的对话不变：裁的是
//! 这一次发给模型的东西。
use super::{agent, rig_model};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use utopia_llm::LlmClient;

type Turns = Vec<(String, String)>;

/// 一轮里三个地方——带工具的请求、RAG 兜底、只看证据的最终作答——共用同一个窗口，
/// 所以是 `Arc<Mutex<_>>`：谁先撞上超限谁来裁，裁完别处照着发，一轮合计只裁一次
#[derive(Clone)]
pub(super) struct Context(Arc<Mutex<Window>>);

struct Window {
    /// 落库的历史，最旧在前；`start` 之前的不再发
    turns: Turns,
    /// 上一轮的工具往返：带 tool_calls 的助手消息和配套的 tool 结果，协议 JSON
    exchange: Vec<Value>,
    /// 最后一条助手消息在 `turns` 里的下标，往返插在它前面
    last_assistant: Option<usize>,
    start: usize,
    /// 往返还发不发。**它单独一位，而且先于任何问答被丢**：一条 get_document 的结果就能
    /// 有 24,000 字，两条就超过缺省预算。从前它和最后那条回答绑在一起，丢它就得把最旧
    /// 到最新的问答一起丢，于是「再短一点」这样的追问不带任何历史发出去（#973 评审）。
    /// 回答是人读过的对话，往返只是那条回答消化过的证据
    exchange_kept: bool,
    /// 这一轮恢复过没有，三个地方合计
    retried: bool,
}

impl Window {
    fn size(&self) -> usize {
        self.turns[self.start..]
            .iter()
            .map(|(_, s)| s.chars().count())
            .sum::<usize>()
            + if self.keeps_exchange() {
                self.exchange
                    .iter()
                    .map(|m| m.to_string().chars().count())
                    .sum()
            } else {
                0
            }
    }

    /// 往返随它的回答走：回答被裁掉了，往返也不能单独留下
    fn keeps_exchange(&self) -> bool {
        self.exchange_kept && self.last_assistant.is_some_and(|i| i >= self.start)
    }

    /// 把 `start` 推到下一条用户消息。按条数截的历史可能从一段问答的中间开始，
    /// 落单的助手回答不发，哪怕预算装得下
    fn skip_orphans(&mut self) {
        while self.start < self.turns.len() && self.turns[self.start].0 != "user" {
            self.start += 1;
        }
    }

    /// 先丢往返，再从最旧的问答起整段整段地丢，直到装进预算；一条消息从不截半
    fn trim(&mut self, budget: usize) {
        self.skip_orphans();
        if self.size() > budget && self.keeps_exchange() {
            self.exchange_kept = false;
        }
        while self.start < self.turns.len() && self.size() > budget {
            self.start += 1;
            self.skip_orphans();
        }
    }
}

/// 落库的历史和往返展开成协议消息，和 rig 的 `wire` 走同一条路，数出来的条数才对得上
fn protocol(turns: &[(String, String)], exchange: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in agent::history_messages(turns, exchange) {
        rig_model::push_message(&mut out, &m);
    }
    out
}

impl Context {
    pub fn new(turns: Turns, exchange: Vec<Value>, budget: usize) -> Self {
        let last_assistant = turns.iter().rposition(|(role, _)| role == "assistant");
        let mut window = Window {
            turns,
            exchange,
            last_assistant,
            start: 0,
            exchange_kept: true,
            retried: false,
        };
        window.trim(budget);
        Self(Arc::new(Mutex::new(window)))
    }

    /// 现在还发的历史和往返
    pub fn snapshot(&self) -> (Turns, Vec<Value>) {
        let w = self.0.lock().unwrap();
        (
            w.turns[w.start..].to_vec(),
            if w.keeps_exchange() {
                w.exchange.clone()
            } else {
                vec![]
            },
        )
    }

    /// rig 的消息列表整轮不变：开头是它当初拿到的完整历史，后面跟本轮的问题和本轮的工具
    /// 往返。每次发送前把历史那一段换成裁过的。数的是协议消息而不是对话轮次——一条落库
    /// 的回答能展开成好几条工具消息；system 消息原地不动——认下的实体那条 system 就贴在
    /// 问题前面。上一轮的往返在最后那条回答之前，丢它是从中间抠掉，不是掐头
    pub fn apply(&self, messages: &[Value]) -> Vec<Value> {
        let w = self.0.lock().unwrap();
        let full = protocol(&w.turns, &w.exchange);
        let kept = protocol(
            &w.turns[w.start..],
            if w.keeps_exchange() { &w.exchange } else { &[] },
        );
        let mut out = Vec::with_capacity(messages.len());
        let mut replaced = 0;
        for m in messages {
            if m["role"] == "system" || replaced == full.len() {
                out.push(m.clone());
                continue;
            }
            replaced += 1;
            if replaced == full.len() {
                out.extend(kept.iter().cloned());
            }
        }
        out
    }

    /// 端点说上下文超了：记住它报的窗口，把历史裁到一半再发一次，同一轮只来一次。
    /// **裁不动就不重发**——历史已经空了、问题本身就超长的时候，原样再发只会再挨一次
    /// 同样的 400（#973 评审）
    pub fn recover(&self, client: &LlmClient, err: &anyhow::Error) -> bool {
        if utopia_llm::context_too_long(err).is_none() {
            return false;
        }
        client.remember_context_window(err);
        let mut w = self.0.lock().unwrap();
        if w.retried {
            return false;
        }
        w.retried = true;
        let before = (w.start, w.keeps_exchange());
        let budget = (w.size() / 2).min(client.history_char_budget());
        w.trim(budget);
        if (w.start, w.keeps_exchange()) == before {
            tracing::info!("Context refused and there is no older exchange to drop");
            return false;
        }
        tracing::info!(
            history_chars = w.size(),
            "Context refused; retrying once with older exchanges removed"
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn refusal() -> anyhow::Error {
        anyhow::Error::new(utopia_llm::ContextTooLong {
            status: 400,
            reason: "400 Bad Request".into(),
            detail: "Prompt is too long".into(),
            window: None,
        })
    }

    fn heavy_exchange() -> Vec<Value> {
        vec![
            json!({"role":"assistant","tool_calls":[
                {"id":"old-1","type":"function","function":{"name":"get_document","arguments":"{}"}},
                {"id":"old-2","type":"function","function":{"name":"get_document","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"old-1","content":"x".repeat(24_000)}),
            json!({"role":"tool","tool_call_id":"old-2","content":"y".repeat(24_000)}),
        ]
    }

    #[test]
    fn a_budget_drops_whole_exchanges_including_failed_and_stopped_turns() {
        let turns = vec![
            ("assistant".into(), "orphan".into()),
            ("user".into(), "failed unanswered question".into()),
            ("user".into(), "latest".into()),
            ("assistant".into(), "[stopped]界😀".into()),
        ];
        let budget = "latest[stopped]界😀".chars().count();
        let context = Context::new(turns.clone(), vec![], budget);
        assert_eq!(context.snapshot().0, turns[2..]);
        let smaller = Context::new(turns, vec![], budget - 1);
        assert!(
            smaller.snapshot().0.is_empty(),
            "never cut the answer to make it fit"
        );
    }

    #[test]
    fn the_previous_tool_exchange_goes_before_any_question_or_answer() {
        let turns: Turns = vec![
            ("user".into(), "which documents mention Aurora?".into()),
            (
                "assistant".into(),
                "Two documents mention Aurora [1][2].".into(),
            ),
        ];
        let exchange = heavy_exchange();
        let context = Context::new(turns.clone(), exchange.clone(), 32_000);
        assert_eq!(context.snapshot().0, turns, "the question and answer stay");
        assert!(context.snapshot().1.is_empty(), "the exchange goes first");

        let system = json!({"role":"system","content":"preserve system"});
        let entities = json!({"role":"system","content":"known entities"});
        let current = json!({"role":"user","content":"make it shorter"});
        let current_call = json!({"role":"assistant","tool_calls":[{"id":"new-call"}]});
        let current_result =
            json!({"role":"tool","tool_call_id":"new-call","content":"preserve evidence"});
        let mut messages = vec![system.clone()];
        messages.extend(protocol(&turns, &exchange));
        messages.extend([
            entities.clone(),
            current.clone(),
            current_call.clone(),
            current_result.clone(),
        ]);
        let mut expected = vec![system];
        expected.extend(protocol(&turns, &[]));
        expected.extend([entities, current, current_call, current_result]);
        assert_eq!(context.apply(&messages), expected);

        let smaller = Context::new(turns.clone(), exchange, 10);
        assert!(smaller.snapshot().0.is_empty(), "then the oldest exchanges");
        assert!(smaller.snapshot().1.is_empty());
    }

    #[test]
    fn a_small_exchange_stays_with_its_answer() {
        let turns: Turns = vec![
            ("user".into(), "old".into()),
            ("assistant".into(), "answer".into()),
            ("user".into(), "failed".into()),
        ];
        let exchange = vec![
            json!({"role":"assistant","tool_calls":[{"id":"c","type":"function","function":{"name":"find_entities","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"c","content":"small"}),
        ];
        let context = Context::new(turns.clone(), exchange.clone(), 32_000);
        assert_eq!(context.snapshot(), (turns.clone(), exchange.clone()));
        let messages = protocol(&turns, &exchange);
        assert_eq!(context.apply(&messages), messages, "nothing to remove");
    }

    #[test]
    fn a_refusal_halves_the_history_once_and_only_when_something_can_go() {
        let client = LlmClient::new("http://unused", None, "m");
        let turns: Turns = (0..4)
            .flat_map(|i| {
                [
                    ("user".into(), format!("question {i}")),
                    ("assistant".into(), "界".repeat(1000)),
                ]
            })
            .collect();
        let context = Context::new(turns.clone(), heavy_exchange(), 32_000);
        assert!(context.snapshot().1.is_empty());
        assert!(context.recover(&client, &refusal()));
        let (kept, _) = context.snapshot();
        assert_eq!(kept, turns[4..], "half of four exchanges, the newest kept");
        assert!(
            !context.recover(&client, &refusal()),
            "the second refusal of a turn is final"
        );

        let empty = Context::new(vec![], vec![], 32_000);
        assert!(
            !empty.recover(&client, &refusal()),
            "nothing to drop, so the same request is not sent again"
        );
        let other = anyhow::anyhow!("network down");
        assert!(!Context::new(turns, vec![], 32_000).recover(&client, &other));
    }
}
