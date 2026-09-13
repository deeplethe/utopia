"""抓合同链语料：按 edgar-manifest.json 从 SEC EDGAR 取原件。

一条链是一份原合同，加上后来改写它的补充协议、物业转让与终止协议。每份存原始 HTML，
旁边放一份纯文本便于阅读与比对。

**文件不进仓库。** 它们是各公司提交给 SEC 的文件，公开可取但版权不属于我们，与
`fetch-sec-filings.mjs` 抓的英伟达语料同一个处理：仓库里只有脚本和清单，跑一次就有。

用法：
    SEC_USER_AGENT="你的名字 you@example.com" py -3 fetch.py [--with-redacted]

SEC 要求 User-Agent 带能联系上的地址，限速每秒 10 次；这个脚本每秒约 3 次。
CSG 与 Comcast 那条链金额涂黑、主协议约 4 MB，默认不抓，要 `--with-redacted`。
"""

import html
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "edgar"
UA = os.environ.get("SEC_USER_AGENT", "Utopia-bench (bench@example.com)")


def url_of(doc):
    acc = doc["accession"].replace("-", "")
    return f"https://www.sec.gov/Archives/edgar/data/{int(doc['cik'])}/{acc}/{doc['file']}"


def to_text(raw: bytes) -> str:
    t = raw.decode("utf-8", "replace")
    t = re.sub(r"(?is)<(script|style).*?</\1>", " ", t)
    t = re.sub(r"(?i)<br\s*/?>|</(p|div|tr|li|h[1-6])>", "\n", t)
    t = re.sub(r"<[^>]+>", " ", t)
    t = html.unescape(t).replace("\xa0", " ")
    t = re.sub(r"[ \t]+", " ", t)
    return re.sub(r"\n\s*\n+", "\n\n", t).strip() + "\n"


def fetch(url: str) -> bytes:
    # 走 curl 不走 urllib：与 fetch-sec-filings.mjs 同一个理由，本机的代理只有 curl 读
    code = b""
    for attempt in range(4):
        r = subprocess.run(
            ["curl", "-sSL", "--max-time", "120", "-H", f"User-Agent: {UA}", "-w", "%{http_code}", url],
            capture_output=True,
        )
        body, code = r.stdout[:-3], r.stdout[-3:]
        if code == b"200" and body:
            return body
        time.sleep(3 + attempt * 5)
    raise RuntimeError(f"failed after retries: {url} ({code!r})")


def main():
    with_redacted = "--with-redacted" in sys.argv
    manifest = json.loads((HERE / "edgar-manifest.json").read_text(encoding="utf-8"))
    total = 0
    for chain in manifest["chains"]:
        if chain.get("redacted") and not with_redacted:
            print(f"skip {chain['id']}（涂黑，要抓就加 --with-redacted）")
            continue
        folder = OUT / chain["id"]
        folder.mkdir(parents=True, exist_ok=True)
        for doc in chain["docs"]:
            stem = f"{doc['seq']:02d}-{doc['role']}-{doc['filed']}"
            dest = folder / f"{stem}.html"
            if dest.exists() and dest.stat().st_size > 0:
                total += 1
                continue
            body = fetch(url_of(doc))
            dest.write_bytes(body)
            (folder / f"{stem}.txt").write_text(to_text(body), encoding="utf-8")
            total += 1
            print(f"ok   {chain['id']}/{stem}  {len(body):>8} bytes")
            time.sleep(0.35)
    print(f"\n{total} 份文件在 {OUT}")


if __name__ == "__main__":
    main()
