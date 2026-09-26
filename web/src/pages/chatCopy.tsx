/* 对话里的复制（#936）：一段回答一个，每个代码块一个。
   控件是外壳里的 IconButton；复制本身走 copyText，局域网里用 http 打开的部署上
   剪贴板 API 不存在，它会退回老办法，两样都不成就说出来。 */
import { useEffect, useRef, useState, type ComponentProps } from "react";
import { Check, Copy } from "lucide-react";
import { copyText } from "../clipboard";
import { S } from "../i18n";
import { toast } from "../toast";
import { IconButton } from "../ui";

/** 复制按钮。`text` 在按下时才取：代码块的字要从渲染好的 DOM 里读 */
export function CopyButton({ label, text }: { label: string; text: () => string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  return (
    <IconButton
      size="sm"
      label={copied ? S.ask.copied : label}
      onClick={async () => {
        if (await copyText(text())) setCopied(true);
        else toast.error(S.ask.copyFailed);
      }}
    >
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </IconButton>
  );
}

/** 回答里的代码块，右上角一个复制。复制的是代码本身：`<pre>` 里的字，
 *  高亮拆出来的那些 span 不影响它；围栏内容末尾那个换行不带走 */
export function CodeBlock(props: ComponentProps<"pre">) {
  const pre = useRef<HTMLPreElement>(null);
  return (
    <div className="u-codeblock">
      <pre {...props} ref={pre} />
      <div className="absolute top-1 right-1">
        <CopyButton
          label={S.ask.copyCode}
          text={() => (pre.current?.textContent ?? "").replace(/\n$/, "")}
        />
      </div>
    </div>
  );
}
