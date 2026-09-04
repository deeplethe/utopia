# GLiNER 对照：同一份语料、同一份答案，量它在实体级上比我们如何。
#
# 两种设置各跑一遍，读数要一起看：
#   --labels truth   只给答案里出现的类（二十来个）。这是它的上限，不是它面对真实本体的水平
#   --labels all     本体全部类分批各跑一遍，同一片段取全局最高分。这才是同等难度
#
# 中文按字切开再喂：GLiNER 按空白分词，不切的话整句是一个 token。切开之后它的最大片段
# 宽度（12 token）就成了 12 个字，更长的名字结构性地找不到。
#
# 用法：
#   set HF_HUB_DISABLE_XET=1        # 镜像不支持 Xet；国内直连很慢时再加 HF_ENDPOINT=https://hf-mirror.com
#   py -3 scripts/bench/gliner_compare.py --corpus pharma --labels truth
#   py -3 scripts/bench/gliner_compare.py --corpus pharma --labels all --classes classes.txt
# classes.txt 一行一个类 key，从要对照的库里导：SELECT key FROM entity_types WHERE kb_id=...
import argparse, io, json, os, re, sys, time
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")
HERE = os.path.dirname(os.path.abspath(__file__))
ap = argparse.ArgumentParser()
ap.add_argument("--corpus", required=True)
ap.add_argument("--labels", choices=["truth", "all"], default="truth")
ap.add_argument("--classes", help="--labels all 时的类 key 清单文件")
ap.add_argument("--model", default="urchade/gliner_multi-v2.1")
ap.add_argument("--threshold", type=float, default=0.3)
ap.add_argument("--batch", type=int, default=25, help="每批标签数；它训练时每条样本只有二三十个")
a = ap.parse_args()

CJK = re.compile(r"([\u4e00-\u9fff\u3000-\u303f\uff00-\uffef])")
def segment(t): return re.sub(r"\s+", " ", CJK.sub(r" \1 ", t)).strip()
def unseg(s): return re.sub(r" (?=[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef])|(?<=[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef]) ", "", s)

corpus = json.load(open(os.path.join(HERE, "corpora", a.corpus + ".json"), encoding="utf-8"))
truth = json.load(open(os.path.join(HERE, "truth", a.corpus + ".json"), encoding="utf-8"))["expect"]
if a.labels == "truth":
    keys = sorted({k for v in truth.values() for k in v} | {"person"})
else:
    keys = [l.strip() for l in open(a.classes, encoding="utf-8") if l.strip()]
labels = [k.replace("_", " ") for k in keys]; back = dict(zip(labels, keys))
print("labels:", len(labels), "batches:", (len(labels) + a.batch - 1) // a.batch, flush=True)

from gliner import GLiNER
model = GLiNER.from_pretrained(a.model)
best = {}
t0 = time.time()
for fname, text in corpus["docs"]:
    seg = segment(text)
    for i in range(0, len(labels), a.batch):
        for e in model.predict_entities(seg, labels[i:i + a.batch], threshold=a.threshold):
            k = (fname, unseg(e["text"]))
            if e["score"] > best.get(k, (0, None))[0]:
                best[k] = (e["score"], back[e["label"]])
preds = sorted({(t, v[1]) for (_, t), v in best.items()})

# 打分规则与 run.mjs 相同：片段包含匹配；答案为空数组的只计找没找到
hit = miss = absent = untyped = 0; notes = []
for frag, accept in truth.items():
    found = [(n, k) for n, k in preds if frag in n]
    if not found: absent += 1; notes.append("absent: " + frag); continue
    if not accept: untyped += 1; continue
    if any(k in accept for _, k in found): hit += 1
    else: miss += 1; notes.append(f"{frag}: 期望 {'|'.join(accept)}，实得 {'/'.join(k for _, k in found)}")
print(json.dumps(preds, ensure_ascii=False))
print(json.dumps({"corpus": a.corpus, "labels": a.labels, "label_count": len(labels), "threshold": a.threshold,
                  "hit": hit, "miss": miss, "absent": absent, "found_no_class_expected": untyped,
                  "total": len(truth), "unique_pairs": len(preds), "seconds": round(time.time() - t0, 1)}, ensure_ascii=False))
for n in notes: print("  ", n)
