//! 一次对话轮次恰好有一个**挣来的**终结（#857）。
//!
//! 这不是为某一个洞写的回归。#845 / #850 / #851 / #852 各自堵住一处「下面失败了、
//! 上面报成功」，每条都带着自己的回归——**四个各抓一个的测试，抓不住第五个**。
//! 第五个会被下一个人用同样的方式写出来：在第一个字节发出之后又加了一条会失败的路，
//! 而「循环跑完了」依然够得着 `done`。
//!
//! 所以这里钉的是规矩本身。表里一行是一个注入点，每一行都过同样三条：
//!
//!   1. 客户端收不到 `event: done`
//!   2. 恰好观察到一个终结（`done` 与 `error` 加起来正好一次）
//!   3. 那个终结说得出理由：`error` 的 data 是带 `code` 与英文原句的 JSON（0004）
//!
//! **新增一条会失败的路，代价是加一行**；加不出那一行，说明这条路自己也没想清楚
//! 该怎么收尾。评审该盯的就是「新开了会失败的路却没加行」。
//!
//! ## 不在表里的路
//!
//! 有几条路的注入手段是夹具级的，进不了这张按回复脚本排的表，各自在同目录的定向测试里：
//! 检索中途出错要关掉夹具连接池，助手 INSERT 被拒要夹具作用域的触发器——
//! `chat_persistence_tests.rs`；降级回答的请求形状——`chat_fallback_tests.rs`；
//! 流在答案中途被切、终结广播早于注册项移除丢失——`chat_registry_tests.rs`。
//! 新开一条这样的路，先看这三个文件里有没有位置，再考虑新文件。

use super::chat_empty_reply_tests::{fixture, Reply, Scripted};

/// 这一轮该怎么收尾。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ends {
    /// 答案成立：一个 `done`，没有 `error`
    Done,
    /// 答案不成立：一个 `error`，**没有** `done`
    Error,
}

/// 表里的一行：一个注入点。
struct Case {
    /// 出了什么事——断言失败时打的就是这句
    what: &'static str,
    /// 假模型这一轮按这个脚本回话
    replies: Vec<Reply>,
    /// 该怎么收尾
    ends: Ends,
    fallback: bool,
}

/// 数一数这条 SSE 里出现了几个终结。
///
/// 按帧数而不是按子串数：`event: done` 也可能出现在某个 `data:` 的正文里
/// （模型完全可以在答案里讨论 SSE），那不是一个终结
fn terminals(sse: &str) -> (usize, usize, Vec<String>) {
    let (mut done, mut error, mut reasons) = (0usize, 0usize, Vec::new());
    for frame in sse.split("\n\n") {
        let mut kind = "";
        let mut data = String::new();
        for line in frame.split('\n') {
            if let Some(rest) = line.strip_prefix("event:") {
                kind = rest.trim();
            } else if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.trim());
            }
        }
        match kind {
            "done" => done += 1,
            "error" => {
                error += 1;
                reasons.push(data);
            }
            _ => {}
        }
    }
    (done, error, reasons)
}

/// 三条断言，每一行都过同一遍。
fn assert_one_earned_terminal(what: &str, ends: Ends, sse: &str) {
    let (done, error, reasons) = terminals(sse);

    assert_eq!(
        done + error,
        1,
        "{what}：恰好一个终结，实际 done={done} error={error}\n{sse}"
    );

    match ends {
        Ends::Done => assert_eq!(done, 1, "{what}：答案成立时该是 done\n{sse}"),
        Ends::Error => {
            assert_eq!(
                done, 0,
                "{what}：失败不许报成 done——这正是 #857 那一族\n{sse}"
            );
            let reason = reasons.first().map(String::as_str).unwrap_or_default();
            assert!(
                !reason.is_empty(),
                "{what}：终结得说得出理由，不能是个空 error\n{sse}"
            );
            // 理由带 code：界面拿它查措辞，英文原句只给日志与不做本地化的客户端（0004）
            let body: serde_json::Value = serde_json::from_str(reason).unwrap_or_default();
            assert!(
                body["code"].as_str().is_some_and(|c| !c.is_empty())
                    && body["error"].as_str().is_some_and(|e| !e.is_empty()),
                "{what}：error 帧要带 code 和原句：{reason}"
            );
        }
    }
}

/// 一个工具调用，让这一轮走完整的取证路径再收尾。
const TOOL: Reply = Reply::Tool("find_entities", r#"{"name":"Acme"}"#);

// The #845 rows test the exhausted gathering boundary, so they reach it before
// injecting a final candidate; a one-tool fixture would exercise an ordinary turn
// instead. Since #937 an ordinary turn is checked for tool-control text as well,
// in a row of its own.
fn at_budget(candidate: Reply) -> Vec<Reply> {
    let mut replies = vec![TOOL; 6];
    replies.push(candidate);
    replies
}

fn table() -> Vec<Case> {
    vec![
        // 对照行：正常回答必须是 done。没有它，上面那三条断言可以靠
        // 「永远不发 done」自动满足，整张表就是空的
        Case {
            what: "模型正常作答",
            replies: vec![TOOL, Reply::Text("Acme 去年第四季度换了 CFO。")],
            ends: Ends::Done,
            fallback: false,
        },
        // 已修：重试之后仍然是空正文
        Case {
            what: "重试之后正文仍然为空",
            replies: vec![TOOL, Reply::Empty, Reply::Empty],
            ends: Ends::Error,
            fallback: false,
        },
        Case {
            what: "降级模型仍然返回空答案",
            replies: vec![Reply::Http(400), Reply::Http(400), Reply::Empty],
            ends: Ends::Error,
            fallback: true,
        },
        // #845：端点在预算耗尽后把工具控制文本当正文吐出来
        Case {
            what: "最后一轮吐的是裸的工具控制标记",
            replies: at_budget(Reply::Text(
                "<DSMLcalls><DSMLinvoke name=\"search\"></DSMLinvoke></DSMLcalls>",
            )),
            ends: Ends::Error,
            fallback: false,
        },
        // #845：同上，但前面先有一段像样的叙述——分帧边界不该影响判断
        Case {
            what: "叙述之后接上工具控制标记",
            replies: at_budget(Reply::Text(
                "我去核对一下证据。\n<DSMLcalls><DSMLinvoke name=\"search\"></DSMLinvoke></DSMLcalls>",
            )),
            ends: Ends::Error,
            fallback: false,
        },
        // #937：普通回合把工具调用写成正文，退回一次之后又写了一次
        Case {
            what: "普通回合连续两次把工具调用写成正文",
            replies: vec![
                TOOL,
                Reply::Text("<DSMLcalls><DSMLinvoke name=\"search\"></DSMLinvoke></DSMLcalls>"),
                Reply::Text("<DSMLcalls><DSMLinvoke name=\"search\"></DSMLinvoke></DSMLcalls>"),
            ],
            ends: Ends::Error,
            fallback: false,
        },
    ]
}

#[tokio::test]
async fn a_turn_ends_in_exactly_one_earned_terminal() -> anyhow::Result<()> {
    let mut ran = 0usize;

    for case in table() {
        let Some(f) = fixture(Scripted::new(case.replies.clone())).await? else {
            eprintln!("没有 UTOPIA_DATABASE_URL，整张表跳过");
            return Ok(());
        };
        let sse = f.ask("Acme 去年第四季度有什么变化？").await?;
        assert_one_earned_terminal(case.what, case.ends, &sse);
        if case.fallback {
            let requests = f.requests();
            assert!(
                requests.len() > 1 && requests.last().is_some_and(|r| r.get("tools").is_none()),
                "{}：必须真的进入无工具的降级回答请求：{requests:?}",
                case.what
            );
        }
        if case.ends == Ends::Error {
            assert!(
                f.stored_answer().await?.is_none(),
                "{}：失败时不能保存助手消息",
                case.what
            );
        }
        eprintln!("verified terminal contract: {}", case.what);
        f.cleanup().await?;
        ran += 1;
    }

    assert!(ran > 0, "整张表被跳空了，等于没测");
    Ok(())
}

/// 终结计数按帧算，不按子串算。
///
/// 单列出来是因为它是上面三条断言的地基：`terminals` 要是把答案正文里的
/// `event: done` 数进去，整张表就会在模型讨论 SSE 的那天集体变绿或集体变红
#[test]
fn a_terminal_is_a_frame_not_a_substring() {
    let sse = "event: delta\ndata: {\"text\": \"SSE 里用 event: done 表示结束\"}\n\n\
               event: done\ndata: {}\n\n";
    assert_eq!(terminals(sse).0, 1, "正文里提到的 done 不算终结");

    let sse = "event: error\ndata: Model returned an empty answer\n\n";
    let (done, error, reasons) = terminals(sse);
    assert_eq!((done, error), (0, 1));
    assert_eq!(reasons, vec!["Model returned an empty answer".to_string()]);
}
