//! One history window and one context-overflow recovery for an entire user turn.
//! Only completed/failed earlier exchanges are removable; current tools are never
//! replayed, and neither the current question nor the system prompt is shortened.
use super::{agent, rig_model};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use utopia_llm::LlmClient;

type Turns = Vec<(String, String)>;

#[derive(Clone)]
pub(super) struct Context(Arc<Mutex<Window>>);

struct Window {
    turns: Turns,
    exchange: Vec<Value>,
    last_assistant: Option<usize>,
    start: usize,
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

    fn keeps_exchange(&self) -> bool {
        self.last_assistant.is_some_and(|i| i >= self.start)
    }

    fn trim(&mut self, budget: usize) {
        // A count-limited DB read may begin halfway through an exchange. Do not
        // send its orphan assistant answer, even when the size budget allows it.
        while self.start < self.turns.len()
            && (self.turns[self.start].0 != "user" || self.size() > budget)
        {
            self.start += 1;
            while self.start < self.turns.len() && self.turns[self.start].0 != "user" {
                self.start += 1;
            }
        }
    }
}

impl Context {
    pub fn new(turns: Turns, exchange: Vec<Value>, budget: usize) -> Self {
        let last_assistant = turns.iter().rposition(|(role, _)| role == "assistant");
        let mut window = Window {
            turns,
            exchange,
            last_assistant,
            start: 0,
            retried: false,
        };
        window.trim(budget);
        Self(Arc::new(Mutex::new(window)))
    }

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

    /// Rig retains its original history across tool turns. Remove the same prefix
    /// every time, leaving all CURRENT tool calls/results in protocol order. Count
    /// protocol messages, not chat turns: one stored answer can replay many tools.
    pub fn apply(&self, messages: &[Value]) -> Vec<Value> {
        let w = self.0.lock().unwrap();
        let removed_exchange = if w.keeps_exchange() {
            &[][..]
        } else {
            &w.exchange
        };
        let mut removed = Vec::new();
        for m in agent::history_messages(&w.turns[..w.start], removed_exchange) {
            rig_model::push_message(&mut removed, &m);
        }
        let mut remaining = removed.len();
        messages
            .iter()
            .filter(|m| {
                if remaining > 0 && m["role"] != "system" {
                    remaining -= 1;
                    false
                } else {
                    true
                }
            })
            .cloned()
            .collect()
    }

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
        let budget = (w.size() / 2).min(client.history_char_budget());
        w.trim(budget);
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
    fn the_previous_tool_exchange_costs_budget_and_leaves_no_orphan_results() {
        let turns = vec![
            ("user".into(), "old".into()),
            ("assistant".into(), "answer".into()),
        ];
        let exchange = vec![
            json!({"role":"assistant","tool_calls":[{"id":"old-call","type":"function","function":{"name":"get_document","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"old-call","content":"x".repeat(1000)}),
        ];
        let context = Context::new(turns.clone(), exchange.clone(), 100);
        assert!(context.snapshot().0.is_empty());
        assert!(context.snapshot().1.is_empty());
        let system = json!({"role":"system","content":"preserve system"});
        let current = json!({"role":"user","content":"old"}); // Same text, different turn.
        let current_call = json!({"role":"assistant","tool_calls":[{"id":"new-call"}]});
        let current_result =
            json!({"role":"tool","tool_call_id":"new-call","content":"preserve evidence"});
        let mut messages = vec![system.clone()];
        for m in agent::history_messages(&turns, &exchange) {
            rig_model::push_message(&mut messages, &m);
        }
        messages.extend([
            current.clone(),
            current_call.clone(),
            current_result.clone(),
        ]);
        assert_eq!(
            context.apply(&messages),
            vec![system, current, current_call, current_result]
        );
    }
}
