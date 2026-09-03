//! #247：来源的种类只在一处定义，前后端对表。
//!
//! 后端 `SourceKind`（utopia-core）一个枚举出两份清单：创建时的白名单、同步时的分派
//! （后者按枚举穷举匹配，编译器保证加了种类就得决定它怎么同步）。前端那一份在
//! `web/src/sourceKinds.ts`，这个测试把它读出来跟枚举比——此前两边各自手写，五种
//! 连接器进了界面、进了同步，却没进创建白名单，界面上选得到、建的时候报
//! 「kind must be one of…」。单元测试与 tsc 都看不见的那种漂移，这里看得见。
//!
//! 不需要数据库。

use std::path::Path;
use utopia_core::models::SourceKind;

/// 从 `CREATABLE_SOURCE_KINDS = [ "…", … ] as const` 里把引号里的字面量按顺序读出来
fn frontend_kinds(src: &str) -> Vec<String> {
    let start = src
        .find("CREATABLE_SOURCE_KINDS = [")
        .expect("web/src/sourceKinds.ts declares CREATABLE_SOURCE_KINDS");
    let body = &src[start..];
    let end = body.find(']').expect("the array closes");
    body[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn the_frontend_list_matches_the_backend_enum() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/sourceKinds.ts");
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let frontend = frontend_kinds(&src);
    let backend: Vec<String> = SourceKind::creatable()
        .map(|k| k.as_str().to_string())
        .collect();
    assert_eq!(
        frontend, backend,
        "web/src/sourceKinds.ts and utopia_core::models::SourceKind list different kinds (order matters: it is the dialog's order)"
    );
}

#[test]
fn every_kind_round_trips_through_its_string() {
    for k in SourceKind::all() {
        assert_eq!(SourceKind::parse(k.as_str()), Some(k), "{k:?}");
    }
    assert_eq!(SourceKind::parse("watch_folder"), None);
    assert!(!SourceKind::Memory.creatable_by_hand());
    assert!(!SourceKind::Upload.creatable_by_hand());
    assert!(SourceKind::S3.creatable_by_hand());
}
