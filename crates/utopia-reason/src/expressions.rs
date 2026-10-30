//! The expression tree shared by rules and column conversions (0032, 0036).
//!
//! CASE is a code-to-value map, not another predicate language. Its arms are
//! literals, so every attribute the tree reads is a premise of every result.

use crate::rules::Arith;
use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};
use uuid::Uuid;

/// The existing bound counts edges from the root, not nodes.
pub const MAX_EXPR_DEPTH: usize = 4;
pub const MAX_CASE_ARMS: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Number(f64),
    Text(String),
}

impl Scalar {
    pub fn read(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => n.as_f64().filter(|n| n.is_finite()).map(Self::Number),
            Value::String(s) => Some(Self::Text(s.clone())),
            _ => None,
        }
    }

    /// Numeric strings remain valid readings, as they were before conversions.
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Text(s) => s.trim().parse().ok(),
        }
        .filter(|n: &f64| n.is_finite())
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Number(n) => json!(n),
            Self::Text(s) => json!(s),
        }
    }

    fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePart {
    Year,
    Month,
    Day,
}

impl DatePart {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "year" => Self::Year,
            "month" => Self::Month,
            "day" => Self::Day,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
        }
    }

    fn truncate(self, value: Scalar) -> Option<Scalar> {
        let Scalar::Text(text) = value else {
            return None;
        };
        // 年精度存的是 "2024"，月精度是 "2024-03"（规则 3）。截到比字更细的一档
        // 是在补一个原文没有的日或月，所以没有。
        let (date, finest) = if text.len() == 10 {
            (
                NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok()?,
                Self::Day,
            )
        } else if text.len() == 7 {
            (
                NaiveDate::parse_from_str(&format!("{text}-01"), "%Y-%m-%d").ok()?,
                Self::Month,
            )
        } else if text.len() == 4 {
            let year: i32 = text.parse().ok()?;
            (NaiveDate::from_ymd_opt(year, 1, 1)?, Self::Year)
        } else {
            // An offset is required: the host timezone must not change a fact.
            (
                chrono::DateTime::parse_from_rfc3339(&text)
                    .ok()?
                    .with_timezone(&chrono::Utc)
                    .date_naive(),
                Self::Day,
            )
        };
        let rank = |part: Self| match part {
            Self::Year => 0,
            Self::Month => 1,
            Self::Day => 2,
        };
        if rank(self) > rank(finest) {
            return None;
        }
        let date = match self {
            Self::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1)?,
            Self::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?,
            Self::Day => date,
        };
        Some(Scalar::Text(date.format("%Y-%m-%d").to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Attr(Uuid),
    Const(f64),
    Text(String),
    Arith {
        op: Arith,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    Number(Box<Expr>),
    Case {
        expr: Box<Expr>,
        arms: Vec<(Scalar, Scalar)>,
        otherwise: Option<Scalar>,
    },
    DateTrunc {
        part: DatePart,
        expr: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprError {
    TooDeep,
    Invalid(&'static str),
}

impl ExprError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooDeep => "expression_too_deep",
            Self::Invalid(_) => "bad_expression",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::TooDeep => "That expression nests too deeply; a rule computes with a few operations, not a program.",
            Self::Invalid(message) => message,
        }
    }
}

impl Expr {
    /// One decoder for write validation and materialization. Legacy numeric
    /// strings keep their meaning; text labels have a separate node.
    pub fn from_json(raw: &Value) -> Result<Self, ExprError> {
        Self::decode(raw, 0)
    }

    fn decode(raw: &Value, depth: usize) -> Result<Self, ExprError> {
        if depth > MAX_EXPR_DEPTH {
            return Err(ExprError::TooDeep);
        }
        let bad = ExprError::Invalid("That expression contains an invalid or unsupported node.");
        let obj = raw.as_object().ok_or_else(|| bad.clone())?;
        // A mixed shape must not acquire meaning from the order of these checks.
        // The page treats unknown fields as unknown too, rather than hiding them.
        if let Some(a) = obj.get("attr") {
            if obj.len() != 1 {
                return Err(bad);
            }
            return Ok(Self::Attr(
                a.as_str().and_then(|s| s.parse().ok()).ok_or(bad)?,
            ));
        }
        if let Some(c) = obj.get("const") {
            if obj.len() != 1 {
                return Err(bad);
            }
            let n = Scalar::read(c).and_then(|s| s.number()).ok_or(bad)?;
            return Ok(Self::Const(n));
        }
        if let Some(text) = obj.get("text").filter(|_| obj.len() == 1) {
            return Ok(Self::Text(text.as_str().ok_or(bad)?.to_owned()));
        }
        if obj.get("cast").and_then(Value::as_str) == Some("number") && obj.len() == 2 {
            return Ok(Self::Number(Box::new(Self::decode(
                obj.get("expr").ok_or(bad)?,
                depth + 1,
            )?)));
        }
        if let Some(part) = obj.get("date_trunc").filter(|_| obj.len() == 2) {
            return Ok(Self::DateTrunc {
                part: part
                    .as_str()
                    .and_then(DatePart::parse)
                    .ok_or_else(|| bad.clone())?,
                expr: Box::new(Self::decode(obj.get("expr").ok_or(bad)?, depth + 1)?),
            });
        }
        if let Some(selector) = obj.get("case") {
            if obj.len() != 2 + usize::from(obj.contains_key("else")) {
                return Err(bad);
            }
            let raw_arms = obj
                .get("when")
                .and_then(Value::as_array)
                .ok_or_else(|| bad.clone())?;
            if raw_arms.is_empty() || raw_arms.len() > MAX_CASE_ARMS {
                return Err(ExprError::Invalid(
                    "A case needs between one and 32 literal arms.",
                ));
            }
            let mut arms = Vec::with_capacity(raw_arms.len());
            for arm in raw_arms {
                let arm = arm
                    .as_object()
                    .filter(|a| a.len() == 2)
                    .ok_or_else(|| bad.clone())?;
                let key = arm
                    .get("is")
                    .and_then(Scalar::read)
                    .ok_or_else(|| bad.clone())?;
                let value = arm
                    .get("then")
                    .and_then(Scalar::read)
                    .ok_or_else(|| bad.clone())?;
                arms.push((key, value));
            }
            let otherwise = obj
                .get("else")
                .map(|v| Scalar::read(v).ok_or(bad))
                .transpose()?;
            let first = &arms[0].1;
            if !arms.iter().all(|(_, v)| first.same_type(v))
                || otherwise.as_ref().is_some_and(|v| !first.same_type(v))
            {
                return Err(ExprError::Invalid(
                    "A case's results must all be numbers or all be text.",
                ));
            }
            return Ok(Self::Case {
                expr: Box::new(Self::decode(selector, depth + 1)?),
                arms,
                otherwise,
            });
        }
        if obj.len() != 3 {
            return Err(bad);
        }
        let op = obj
            .get("op")
            .and_then(Value::as_str)
            .and_then(Arith::parse)
            .ok_or_else(|| bad.clone())?;
        Ok(Self::Arith {
            op,
            l: Box::new(Self::decode(
                obj.get("l").ok_or_else(|| bad.clone())?,
                depth + 1,
            )?),
            r: Box::new(Self::decode(obj.get("r").ok_or(bad)?, depth + 1)?),
        })
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Attr(id) => json!({"attr": id}),
            Self::Const(n) => json!({"const": n}),
            Self::Text(s) => json!({"text": s}),
            Self::Arith { op, l, r } => {
                json!({"op": op.as_str(), "l": l.to_json(), "r": r.to_json()})
            }
            Self::Number(expr) => json!({"cast": "number", "expr": expr.to_json()}),
            Self::DateTrunc { part, expr } => {
                json!({"date_trunc": part.as_str(), "expr": expr.to_json()})
            }
            Self::Case {
                expr,
                arms,
                otherwise,
            } => {
                let arms: Vec<_> = arms
                    .iter()
                    .map(|(key, value)| json!({"is": key.to_json(), "then": value.to_json()}))
                    .collect();
                let mut out = json!({"case": expr.to_json(), "when": arms});
                if let Some(value) = otherwise {
                    out["else"] = value.to_json();
                }
                out
            }
        }
    }

    /// Attribute IDs in encounter order, without duplicates.
    pub fn predicates(&self, out: &mut Vec<Uuid>) {
        match self {
            Self::Attr(p) => {
                if !out.contains(p) {
                    out.push(*p);
                }
            }
            Self::Const(_) | Self::Text(_) => {}
            Self::Arith { l, r, .. } => {
                l.predicates(out);
                r.predicates(out);
            }
            Self::Number(expr) | Self::Case { expr, .. } | Self::DateTrunc { expr, .. } => {
                expr.predicates(out)
            }
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Self::Attr(_) | Self::Const(_) | Self::Text(_) => 1,
            Self::Arith { l, r, .. } => 1 + l.depth().max(r.depth()),
            Self::Number(expr) | Self::Case { expr, .. } | Self::DateTrunc { expr, .. } => {
                1 + expr.depth()
            }
        }
    }

    /// A failed conversion is absent, just like a missing reading or division
    /// by zero. A caller must not turn it into a zero or a successful fact.
    pub fn evaluate(&self, read: &impl Fn(Uuid) -> Option<Scalar>) -> Option<Scalar> {
        match self {
            Self::Attr(p) => read(*p),
            Self::Const(n) => n.is_finite().then_some(Scalar::Number(*n)),
            Self::Text(s) => Some(Scalar::Text(s.clone())),
            Self::Number(expr) => expr.evaluate(read)?.number().map(Scalar::Number),
            Self::Arith { op, l, r } => {
                let (a, b) = (l.evaluate(read)?.number()?, r.evaluate(read)?.number()?);
                let value = match op {
                    Arith::Add => a + b,
                    Arith::Sub => a - b,
                    Arith::Mul => a * b,
                    Arith::Div if b != 0.0 => a / b,
                    Arith::Div => return None,
                };
                value.is_finite().then_some(Scalar::Number(value))
            }
            Self::Case {
                expr,
                arms,
                otherwise,
            } => {
                let key = expr.evaluate(read)?;
                arms.iter()
                    .find(|(candidate, _)| *candidate == key)
                    .map(|(_, value)| value.clone())
                    .or_else(|| otherwise.clone())
            }
            Self::DateTrunc { part, expr } => part.truncate(expr.evaluate(read)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{
        evaluate, AttrFact, BusinessRule, Conclusion, Condition, Op, Operand, Side,
    };
    use std::collections::HashMap;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn conversions_round_trip_and_keep_only_their_input_references() {
        let raw = json!({"op":"div", "l":{"cast":"number","expr":{"attr":id(1)}},
            "r":{"case":{"attr":id(2)},"when":[{"is":"cents","then":100}],"else":1}});
        let expr = Expr::from_json(&raw).unwrap();
        let mut reads = Vec::new();
        expr.predicates(&mut reads);
        assert_eq!(reads, [id(1), id(2)]);
        let values = HashMap::from([
            (id(1), Scalar::Text("12550".into())),
            (id(2), Scalar::Text("cents".into())),
        ]);
        assert_eq!(
            expr.evaluate(&|p| values.get(&p).cloned()),
            Some(Scalar::Number(125.5))
        );
        assert_eq!(Expr::from_json(&expr.to_json()).unwrap(), expr);
        assert_eq!(
            Expr::from_json(&json!({"const":"2.50"})).unwrap(),
            Expr::Const(2.5)
        );
        assert_eq!(
            Expr::from_json(&json!({"text":"2.50"})).unwrap(),
            Expr::Text("2.50".into())
        );
    }

    #[test]
    fn missing_or_invalid_values_never_become_a_default_or_zero() {
        let cast = Expr::from_json(&json!({"cast":"number","expr":{"attr":id(1)}})).unwrap();
        for value in [
            Value::Null,
            json!(false),
            json!(""),
            json!("NaN"),
            json!("Infinity"),
            json!("1e999"),
            json!("12 CNY"),
        ] {
            assert_eq!(cast.evaluate(&|_| Scalar::read(&value)), None, "{value}");
        }
        let choice = Expr::from_json(
            &json!({"case":{"attr":id(1)},"when":[{"is":1,"then":"app"}],"else":"other"}),
        )
        .unwrap();
        assert_eq!(choice.evaluate(&|_| None), None);
        assert_eq!(
            choice.evaluate(&|_| Some(Scalar::Number(9.0))),
            Some(Scalar::Text("other".into()))
        );
        let no_else =
            Expr::from_json(&json!({"case":{"attr":id(1)},"when":[{"is":1,"then":"app"}]}))
                .unwrap();
        assert_eq!(no_else.evaluate(&|_| Some(Scalar::Number(9.0))), None);
        // Text codes are not silently coerced to numeric codes by CASE.
        assert_eq!(no_else.evaluate(&|_| Some(Scalar::Text("1".into()))), None);
    }

    #[test]
    fn calendar_truncation_uses_utc_and_rejects_ambiguous_or_invalid_dates() {
        for (part, input, expected) in [
            ("month", "2024-03-01T00:30:00+08:00", "2024-02-01"),
            ("day", "2024-02-29T23:30:00-02:00", "2024-03-01"),
            ("year", "2024-01-01T00:30:00+01:00", "2023-01-01"),
            ("day", "2024-02-29", "2024-02-29"),
            ("month", "2024-02-29", "2024-02-01"),
            ("month", "2024-03", "2024-03-01"),
            ("year", "2024-03", "2024-01-01"),
            ("year", "2024", "2024-01-01"),
        ] {
            let expr = Expr::from_json(&json!({"date_trunc":part,"expr":{"attr":id(1)}})).unwrap();
            assert_eq!(
                expr.evaluate(&|_| Some(Scalar::Text(input.into()))),
                Some(Scalar::Text(expected.into()))
            );
        }
        let expr = Expr::from_json(&json!({"date_trunc":"month","expr":{"attr":id(1)}})).unwrap();
        let day = Expr::from_json(&json!({"date_trunc":"day","expr":{"attr":id(1)}})).unwrap();
        // 月精度没有日，年精度没有月：截得更细就没有值
        assert_eq!(
            day.evaluate(&|_| Some(Scalar::Text("2024-03".into()))),
            None
        );
        assert_eq!(expr.evaluate(&|_| Some(Scalar::Text("2024".into()))), None);
        for input in [
            "2023-02-29",
            "2024-02-30",
            "2024-03-01T00:00:00",
            "yesterday",
            "",
        ] {
            assert_eq!(
                expr.evaluate(&|_| Some(Scalar::Text(input.into()))),
                None,
                "{input}"
            );
        }
    }

    #[test]
    fn every_new_node_obeys_the_depth_and_case_bounds() {
        let mut raw = json!({"attr":id(1)});
        for _ in 0..MAX_EXPR_DEPTH {
            raw = json!({"cast":"number","expr":raw});
        }
        assert!(Expr::from_json(&raw).is_ok());
        assert_eq!(
            Expr::from_json(&json!({"date_trunc":"day","expr":raw})),
            Err(ExprError::TooDeep)
        );
        for raw in [
            json!({"attr":id(1),"cast":"number","expr":{"attr":id(2)}}),
            json!({"const":2,"text":"app"}),
            json!({"op":"add","l":{"attr":id(1)},"r":{"const":1},"future":true}),
            json!({"cast":"integer","expr":{"attr":id(1)}}),
            json!({"date_trunc":"hour","expr":{"attr":id(1)}}),
            json!({"case":{"attr":id(1)},"when":[]}),
            json!({"case":{"attr":id(1)},"when":[{"is":1,"then":"app"}],"else":0}),
            json!({"case":{"attr":id(1)},"when":[{"is":1,"then":{"attr":id(2)}}]}),
            json!({"case":{"attr":id(1)},"when":[{"is":null,"then":"app"}]}),
            json!({"case":{"attr":id(1)},"when":(0..=MAX_CASE_ARMS).map(|n|json!({"is":n,"then":"app"})).collect::<Vec<_>>()}),
        ] {
            assert!(Expr::from_json(&raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn text_results_keep_each_readings_interval_and_proof() {
        let facts = [
            AttrFact {
                id: id(1),
                subject: id(99),
                predicate: id(10),
                value: json!(5),
            },
            AttrFact {
                id: id(2),
                subject: id(99),
                predicate: id(11),
                value: json!(1),
            },
            AttrFact {
                id: id(3),
                subject: id(99),
                predicate: id(11),
                value: json!(2),
            },
        ];
        let spans = HashMap::from([
            (id(1), (Some(10), Some(30))),
            (id(2), (Some(0), Some(20))),
            (id(3), (Some(20), Some(40))),
        ]);
        let rule = BusinessRule {
            id: id(50),
            join_predicate: None,
            conclusion: Conclusion::Computed {
                predicate: id(12),
                expr: Expr::from_json(&json!({
                    "case":{"attr":id(11)},"when":[{"is":1,"then":"app"},{"is":2,"then":"web"}]
                }))
                .unwrap(),
            },
            conditions: vec![Condition {
                group: 0,
                side: Side::X,
                predicate: id(10),
                op: Op::Present,
                operand: Operand::None,
            }],
        };
        let (mut hits, report) = evaluate(&[rule], &facts, &spans, &[]);
        assert_eq!(report.hits, 2);
        hits.sort_by_key(|h| h.from);
        assert_eq!((hits[0].from, hits[0].to), (Some(10), Some(20)));
        assert_eq!(hits[0].value, Some(Scalar::Text("app".into())));
        assert_eq!(hits[0].premises, [id(1), id(2)]);
        assert_eq!((hits[1].from, hits[1].to), (Some(20), Some(30)));
        assert_eq!(hits[1].value, Some(Scalar::Text("web".into())));
        assert_eq!(hits[1].premises, [id(1), id(3)]);
    }
}
