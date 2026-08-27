#!/usr/bin/env bash
# 端到端冒烟测试：注册 → 登录态 → 工作区 → 知识库 → 权限 → 任务队列
# 用法: ./scripts/smoke.sh [BASE_URL]  (默认 http://localhost:8080)
# JSON body 走临时文件，避免 Windows shell 的编码转换问题。
set -euo pipefail

BASE="${1:-http://localhost:8080}"
EMAIL="smoke-$(date +%s)@test.local"
JAR="$(mktemp)"
BODY="$(mktemp)"
trap 'rm -f "$JAR" "$BODY"' EXIT

step() { echo "--- $1"; }
fail() { echo "FAIL: $1" >&2; exit 1; }

step "health"
curl -sf "$BASE/api/v1/health" | grep -q '"ok"' || fail "health"

step "register ($EMAIL)"
printf '{"email":"%s","password":"password123","display_name":"Smoke Tester"}' "$EMAIL" > "$BODY"
curl -sf -c "$JAR" -H 'Content-Type: application/json' --data-binary @"$BODY" \
  "$BASE/api/v1/auth/register" | grep -q '"user"' || fail "register"

step "me (cookie 认证)"
curl -sf -b "$JAR" "$BASE/api/v1/auth/me" | grep -q "$EMAIL" || fail "me"

step "workspaces（应至少含默认工作区）"
curl -sf -b "$JAR" "$BASE/api/v1/workspaces" | grep -q '"id"' || fail "workspace list 为空"

step "创建自己的工作区（成为 owner）"
printf '{"name":"Smoke WS"}' > "$BODY"
WS_ID=$(curl -sf -b "$JAR" -H 'Content-Type: application/json' --data-binary @"$BODY" \
  "$BASE/api/v1/workspaces" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
[ -n "$WS_ID" ] || fail "create workspace"
echo "    workspace: $WS_ID"

step "创建知识库"
printf '{"name":"Smoke KB"}' > "$BODY"
KB_ID=$(curl -sf -b "$JAR" -H 'Content-Type: application/json' --data-binary @"$BODY" \
  "$BASE/api/v1/workspaces/$WS_ID/kbs" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
[ -n "$KB_ID" ] || fail "create kb"
echo "    kb: $KB_ID"

step "上传文档（中英混合 markdown）"
# 注意：不要用 curl 的 ";filename=" 语法——Git Bash 的 MSYS 路径转换会把含分号的参数弄坏
DOCDIR="$(mktemp -d)"
DOC="$DOCDIR/smoke.md"
# “凤凰项目由张三负责。” 以 UTF-8 字节写入，绕开 Windows shell 编码转换
printf '# Phoenix Handbook\n\n\xe5\x87\xa4\xe5\x87\xb0\xe9\xa1\xb9\xe7\x9b\xae\xe7\x94\xb1\xe5\xbc\xa0\xe4\xb8\x89\xe8\xb4\x9f\xe8\xb4\xa3\xe3\x80\x82 Budget is 12 million CNY.\n' > "$DOC"
curl -sf -b "$JAR" -F "files=@$DOC" \
  "$BASE/api/v1/kbs/$KB_ID/documents" | grep -q '"created"' || fail "upload"
rm -rf "$DOCDIR"

step "等待摄入管道完成"
for i in $(seq 1 20); do
  S=$(curl -sf -b "$JAR" "$BASE/api/v1/kbs/$KB_ID/documents" | grep -o '"status":"[^"]*"' | head -1)
  case "$S" in
    *ready*) break;;
    *failed*) fail "文档处理失败";;
  esac
  sleep 2
done
case "$S" in *ready*) ;; *) fail "处理超时（当前 $S）";; esac

step "中文搜索（凤凰项目）"
printf '{"q":"\xe5\x87\xa4\xe5\x87\xb0\xe9\xa1\xb9\xe7\x9b\xae"}' > "$BODY"
curl -sf -b "$JAR" -H 'Content-Type: application/json' --data-binary @"$BODY" \
  "$BASE/api/v1/kbs/$KB_ID/search" | grep -q '"filename":"smoke.md"' || fail "中文搜索未命中"

step "未认证访问应 401"
CODE=$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/v1/workspaces")
[ "$CODE" = "401" ] || fail "预期 401，实际 $CODE"

step "登录（第二会话）"
printf '{"email":"%s","password":"password123"}' "$EMAIL" > "$BODY"
curl -sf -c "$JAR.2" -H 'Content-Type: application/json' --data-binary @"$BODY" \
  "$BASE/api/v1/auth/login" | grep -q '"token"' || fail "login"
rm -f "$JAR.2"

step "入队 noop 任务"
curl -sf -b "$JAR" -X POST "$BASE/api/v1/jobs/noop" | grep -q '"job_id"' || fail "enqueue"

echo "=== 冒烟测试全部通过 ==="
