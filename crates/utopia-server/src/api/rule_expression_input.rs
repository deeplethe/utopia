//! Text is a write-time convenience, never a second stored rule language (0036).
//! Resolve keys, compile an allowlisted scalar expression, then let the ordinary
//! rule writer check the UUID references in the current base again.

use serde_json::Value;
use sqlparser::ast::{
    BinaryOperator, CastKind, Expr as SqlExpr, FunctionArg, FunctionArgExpr, FunctionArguments,
    UnaryOperator, Value as SqlValue,
};
use sqlparser::{dialect::GenericDialect, parser::Parser, tokenizer::Token};
use std::collections::HashMap;
use utopia_core::error::{AppError, AppResult};
use utopia_reason::expressions::{DatePart, Scalar};
use utopia_reason::rules::{Arith, Expr, MAX_EXPR_DEPTH};
use utopia_store::business_rules::ConditionInput;
use uuid::Uuid;

const MAX_INPUT_BYTES: usize = 8192;

pub(super) async fn compile_inputs(
    pool: &sqlx::PgPool,
    kb_id: Uuid,
    conclusion: Option<&mut Value>,
    conditions: &mut [ConditionInput],
) -> AppResult<()> {
    let mut inputs: Vec<&mut Value> = conclusion
        .into_iter()
        .chain(
            conditions
                .iter_mut()
                .filter(|c| matches!(c.op.as_str(), "gt" | "gte" | "lt" | "lte"))
                .filter_map(|c| c.operand.as_mut()),
        )
        .filter(|value| value.get("expression").is_some())
        .collect();
    if inputs.is_empty() {
        // Metadata-only PATCH must not acquire new semantic requirements.
        return Ok(());
    }
    let rows: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT key, id FROM relation_types WHERE kb_id=$1 AND kind='attribute'")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;
    let names = rows.into_iter().collect();
    for value in &mut inputs {
        let text = value
            .as_object()
            .filter(|o| o.len() == 1)
            .and_then(|o| o.get("expression"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                invalid("Text input is an object containing only an expression string.")
            })?;
        **value = compile(text, &names)?;
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::invalid("bad_expression", message)
}

fn unsupported() -> AppError {
    invalid("Use attributes, literals, arithmetic, CAST AS DOUBLE PRECISION, a simple literal CASE, or date_trunc with year, month or day.")
}

pub(crate) fn compile(text: &str, names: &HashMap<String, Uuid>) -> AppResult<Value> {
    if text.trim().is_empty() || text.len() > MAX_INPUT_BYTES {
        return Err(invalid(
            "An expression must contain between one and 8192 bytes.",
        ));
    }
    // Bound parser recursion too: parentheses do not count as expression nodes,
    // but accepting arbitrarily many of them would still consume a parser stack.
    let dialect = GenericDialect {};
    let mut parser = Parser::new(&dialect)
        .with_recursion_limit(32)
        .try_with_sql(text)
        .map_err(|e| invalid(e.to_string()))?;
    let ast = parser.parse_expr().map_err(|e| invalid(e.to_string()))?;
    if parser.peek_token().token != Token::EOF {
        return Err(invalid(
            "Supply one scalar expression, without a statement, alias or trailing clause.",
        ));
    }
    let expr = compile_ast(&ast, names, 0)?;
    let tree = expr.to_json();
    // The JSON and text entry points must accept the same case shapes and depth.
    Expr::from_json(&tree).map_err(|e| AppError::invalid(e.code(), e.message()))?;
    Ok(tree)
}

fn literal(ast: &SqlExpr) -> AppResult<Scalar> {
    match ast {
        SqlExpr::Nested(inner) => literal(inner),
        SqlExpr::Value(v) => match &v.value {
            SqlValue::Number(text, false) => text
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(Scalar::Number)
                .ok_or_else(unsupported),
            SqlValue::SingleQuotedString(text) => Ok(Scalar::Text(text.clone())),
            _ => Err(unsupported()),
        },
        SqlExpr::UnaryOp { op, expr }
            if matches!(op, UnaryOperator::Plus | UnaryOperator::Minus) =>
        {
            let Scalar::Number(n) = literal(expr)? else {
                return Err(unsupported());
            };
            Ok(Scalar::Number(if *op == UnaryOperator::Minus {
                -n
            } else {
                n
            }))
        }
        _ => Err(invalid(
            "CASE keys and results must be literal numbers or text.",
        )),
    }
}

fn compile_ast(ast: &SqlExpr, names: &HashMap<String, Uuid>, depth: usize) -> AppResult<Expr> {
    if depth > MAX_EXPR_DEPTH {
        let error = utopia_reason::expressions::ExprError::TooDeep;
        return Err(AppError::invalid(error.code(), error.message()));
    }
    match ast {
        SqlExpr::Nested(inner) => compile_ast(inner, names, depth),
        SqlExpr::Identifier(name) => {
            names
                .get(&name.value)
                .copied()
                .map(Expr::Attr)
                .ok_or_else(|| {
                    AppError::invalid(
                        "unknown_expression_attribute",
                        format!("There is no attribute key {:?} in this base.", name.value),
                    )
                })
        }
        SqlExpr::Value(_) => match literal(ast)? {
            Scalar::Number(n) => Ok(Expr::Const(n)),
            Scalar::Text(s) => Ok(Expr::Text(s)),
        },
        SqlExpr::BinaryOp { left, op, right } => {
            let op = match op {
                BinaryOperator::Plus => Arith::Add,
                BinaryOperator::Minus => Arith::Sub,
                BinaryOperator::Multiply => Arith::Mul,
                BinaryOperator::Divide => Arith::Div,
                _ => return Err(unsupported()),
            };
            Ok(Expr::Arith {
                op,
                l: Box::new(compile_ast(left, names, depth + 1)?),
                r: Box::new(compile_ast(right, names, depth + 1)?),
            })
        }
        SqlExpr::UnaryOp { op, expr }
            if matches!(op, UnaryOperator::Plus | UnaryOperator::Minus) =>
        {
            if let Ok(Scalar::Number(n)) = literal(ast) {
                return Ok(Expr::Const(n));
            }
            if *op == UnaryOperator::Plus {
                return compile_ast(expr, names, depth);
            }
            Ok(Expr::Arith {
                op: Arith::Sub,
                l: Box::new(Expr::Const(0.0)),
                r: Box::new(compile_ast(expr, names, depth + 1)?),
            })
        }
        SqlExpr::Cast {
            kind,
            expr,
            data_type,
            array,
            format,
        } if matches!(kind, CastKind::Cast | CastKind::DoubleColon)
            && !array
            && format.is_none()
            && matches!(
                data_type.to_string().as_str(),
                "DOUBLE" | "DOUBLE PRECISION"
            ) =>
        {
            Ok(Expr::Number(Box::new(compile_ast(expr, names, depth + 1)?)))
        }
        SqlExpr::Case {
            operand: Some(operand),
            conditions,
            else_result,
            ..
        } => {
            let arms = conditions
                .iter()
                .map(|arm| Ok((literal(&arm.condition)?, literal(&arm.result)?)))
                .collect::<AppResult<_>>()?;
            Ok(Expr::Case {
                expr: Box::new(compile_ast(operand, names, depth + 1)?),
                arms,
                otherwise: else_result.as_ref().map(|v| literal(v)).transpose()?,
            })
        }
        SqlExpr::Function(function)
            if function.name.to_string().eq_ignore_ascii_case("date_trunc") =>
        {
            if function.uses_odbc_syntax
                || !matches!(function.parameters, FunctionArguments::None)
                || function.filter.is_some()
                || function.null_treatment.is_some()
                || function.over.is_some()
                || !function.within_group.is_empty()
            {
                return Err(unsupported());
            }
            let FunctionArguments::List(args) = &function.args else {
                return Err(unsupported());
            };
            if args.duplicate_treatment.is_some() || !args.clauses.is_empty() {
                return Err(unsupported());
            }
            let [FunctionArg::Unnamed(FunctionArgExpr::Expr(part)), FunctionArg::Unnamed(FunctionArgExpr::Expr(expr))] =
                args.args.as_slice()
            else {
                return Err(unsupported());
            };
            let Scalar::Text(part) = literal(part)? else {
                return Err(unsupported());
            };
            let part = DatePart::parse(&part.to_ascii_lowercase()).ok_or_else(unsupported)?;
            Ok(Expr::DateTrunc {
                part,
                expr: Box::new(compile_ast(expr, names, depth + 1)?),
            })
        }
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names() -> HashMap<String, Uuid> {
        ["amt_pay", "chnl", "dt_crt", "净额"]
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name.to_owned(), Uuid::from_u128(i as u128 + 1)))
            .collect()
    }

    #[test]
    fn the_wide_corpus_conversions_compile_to_ids_and_run() {
        let names = names();
        let inputs = HashMap::from([
            (names["amt_pay"], Scalar::Text("12345".into())),
            (names["chnl"], Scalar::Number(2.0)),
            (
                names["dt_crt"],
                Scalar::Text("2024-03-01T00:30:00+08:00".into()),
            ),
            (names["净额"], Scalar::Number(250.0)),
        ]);
        for (text, expected) in [
            ("CAST(amt_pay AS DOUBLE PRECISION) / 100", json!(123.45)),
            (
                "CASE chnl WHEN 1 THEN 'app' WHEN 2 THEN 'web' ELSE 'other' END",
                json!("web"),
            ),
            ("date_trunc('month', dt_crt)", json!("2024-02-01")),
            ("(\"净额\" - 50) / (10 - 2)", json!(25.0)),
            ("-amt_pay / -1e2", json!(123.45)),
            (
                "CASE chnl WHEN 2 THEN 'customer''s app' END",
                json!("customer's app"),
            ),
        ] {
            let tree = compile(text, &names).unwrap_or_else(|e| panic!("{text}: {e}"));
            let expr = Expr::from_json(&tree).unwrap();
            assert_eq!(
                expr.evaluate(&|p| inputs.get(&p).cloned())
                    .unwrap()
                    .to_json(),
                expected,
                "{text}"
            );
            assert_eq!(Expr::from_json(&expr.to_json()).unwrap(), expr);
        }
        assert!(compile("CASE chnl WHEN 3 THEN 'web' END", &names).is_ok());
    }

    #[test]
    fn unsupported_sql_cannot_be_silently_shortened_into_a_conversion() {
        for text in [
            "unknown / 100",
            "other.amt_pay",
            "amt_pay; DELETE FROM jobs",
            "amt_pay AS amount",
            "amt_pay FROM jobs",
            "(SELECT 1)",
            "SUM(amt_pay)",
            "amt_pay % 100",
            "amt_pay > 0",
            "CAST(amt_pay AS DECIMAL(10, 2))",
            "CAST(amt_pay AS INTEGER)",
            "TRY_CAST(amt_pay AS DOUBLE)",
            "CASE WHEN chnl = 1 THEN 'app' END",
            "CASE chnl WHEN 1 THEN amt_pay END",
            "CASE chnl WHEN 1 THEN 'app' ELSE 0 END",
            "CASE chnl WHEN NULL THEN 'app' END",
            "date_trunc('hour', dt_crt)",
            "date_trunc('month', dt_crt, 'UTC')",
            "date_trunc(DISTINCT 'month', dt_crt)",
            "date_trunc('month', dt_crt) OVER ()",
            "date_trunc('month', dt_crt) FILTER (WHERE chnl = 1)",
            "date_trunc('month', dt_crt ORDER BY chnl)",
            "pg_catalog.date_trunc('month', dt_crt)",
            "date_trunc(unit => 'month', value => dt_crt)",
            "1e999",
            "true",
            "NULL",
            "",
            ":parameter",
        ] {
            assert!(compile(text, &names()).is_err(), "accepted {text}");
        }
    }

    #[test]
    fn text_and_json_share_the_existing_depth_bound() {
        let mut text = "amt_pay".to_owned();
        for _ in 0..MAX_EXPR_DEPTH {
            text = format!("({text} + 1)");
        }
        assert!(compile(&text, &names()).is_ok());
        assert!(compile(&format!("{text} + 1"), &names()).is_err());
        assert!(compile(
            &format!("{}amt_pay{}", "(".repeat(1000), ")".repeat(1000)),
            &names()
        )
        .is_err());
        assert!(compile(&" ".repeat(MAX_INPUT_BYTES + 1), &names()).is_err());
    }
}
