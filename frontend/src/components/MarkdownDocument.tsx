import { useEffect, useMemo, useRef } from "react"
import DOMPurify from "dompurify"
import { marked } from "marked"
import mermaid from "mermaid"
import { useTheme } from "next-themes"

function escapeHtml(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;")
}

const renderer = new marked.Renderer()

renderer.code = ({ text, lang }) => {
  if (lang?.trim().toLowerCase() === "mermaid") {
    return `<div class="mermaid">${escapeHtml(text)}</div>`
  }
  const language = lang?.trim() ? ` data-language="${escapeHtml(lang.trim())}"` : ""
  return `<pre${language}><code>${escapeHtml(text)}</code></pre>`
}

renderer.link = ({ href, title, text }) => {
  const safeHref = escapeHtml(href)
  const safeTitle = title ? ` title="${escapeHtml(title)}"` : ""
  const external = /^https?:\/\//i.test(href) ? ' target="_blank" rel="noreferrer"' : ""
  return `<a href="${safeHref}"${safeTitle}${external}>${text}</a>`
}

marked.use({ gfm: true, breaks: false, renderer })

export default function MarkdownDocument({ source, documentId }: { source: string; documentId: string }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const { resolvedTheme } = useTheme()
  const html = useMemo(() => {
    const rendered = (marked.parse(source) as string)
      .replaceAll("<table>", '<div class="docs-table-scroll"><table>')
      .replaceAll("</table>", "</table></div>")
    return DOMPurify.sanitize(rendered, {
      ADD_ATTR: ["target", "rel", "data-language"],
    })
  }, [source])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    const nodes = Array.from(container.querySelectorAll<HTMLElement>(".mermaid"))
    if (!nodes.length) return

    const dark = resolvedTheme === "dark"
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: "strict",
      theme: "base",
      fontFamily: "Manrope Variable, sans-serif",
      themeVariables: {
        background: dark ? "#252321" : "#ffffff",
        primaryColor: dark ? "#173744" : "#e9f4f7",
        primaryTextColor: dark ? "#f5f5f4" : "#1c1917",
        primaryBorderColor: "#3c879f",
        secondaryColor: dark ? "#292f31" : "#f5f8f9",
        secondaryTextColor: dark ? "#e7e5e4" : "#292524",
        secondaryBorderColor: dark ? "#527582" : "#9ab9c4",
        tertiaryColor: dark ? "#302e2b" : "#fafaf9",
        tertiaryTextColor: dark ? "#e7e5e4" : "#44403c",
        tertiaryBorderColor: dark ? "#57534e" : "#d6d3d1",
        lineColor: "#3c879f",
        textColor: dark ? "#f5f5f4" : "#1c1917",
        mainBkg: dark ? "#173744" : "#e9f4f7",
        nodeBorder: "#3c879f",
        clusterBkg: dark ? "#242b2e" : "#f4f9fa",
        clusterBorder: dark ? "#527582" : "#9ab9c4",
        edgeLabelBackground: dark ? "#252321" : "#ffffff",
        fontSize: "14px",
      },
      flowchart: { curve: "basis", htmlLabels: true, padding: 16 },
    })

    let cancelled = false
    void mermaid.run({ nodes, suppressErrors: true }).catch(() => {
      if (!cancelled) nodes.forEach((node) => node.classList.add("mermaid-error"))
    })
    return () => { cancelled = true }
  }, [documentId, html, resolvedTheme])

  return (
    <div
      key={`${documentId}-${resolvedTheme}`}
      ref={containerRef}
      className="docs-markdown"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
