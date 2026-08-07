import { useEffect, useRef, useState } from "react"
import { Check, ChevronsUpDown, Search } from "lucide-react"

export type ComboboxOption = { value: string; label: string; hint?: string }

/**
 * Searchable single-select: a Select-like trigger that opens a dropdown with a search box, so
 * long lists (classes, properties, individuals) are typed-to-filter instead of scrolled. Options
 * are filtered client-side. Theme-tokened (matches the app's other dropdowns).
 */
export function Combobox({
  value, onChange, options, placeholder = "Select…", searchPlaceholder = "Search…",
  disabled, className = "", triggerClassName = "h-9", emptyText = "No matches.",
}: {
  value: string | null
  onChange: (value: string) => void
  options: ComboboxOption[]
  placeholder?: string
  searchPlaceholder?: string
  disabled?: boolean
  className?: string
  triggerClassName?: string
  emptyText?: string
}) {
  const [open, setOpen] = useState(false)
  const [q, setQ] = useState("")
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    if (open) { setQ(""); setTimeout(() => inputRef.current?.focus(), 0) }
  }, [open])

  const selected = options.find((o) => o.value === value)
  const term = q.trim().toLowerCase()
  const matches = (term ? options.filter((o) => o.label.toLowerCase().includes(term)) : options).slice(0, 100)

  return (
    <div className={`relative ${className}`}>
      <button
        type="button" disabled={disabled} onClick={() => setOpen((o) => !o)}
        className={`flex w-full items-center justify-between gap-2 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm ring-offset-background focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 ${triggerClassName}`}
      >
        <span className={`truncate ${selected ? "" : "text-muted-foreground"}`}>{selected?.label ?? placeholder}</span>
        <ChevronsUpDown className="h-4 w-4 shrink-0 opacity-50" />
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute z-50 mt-1 w-full min-w-[12rem] rounded-md border bg-popover p-1 text-popover-foreground shadow-md">
            <div className="relative mb-1">
              <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <input
                ref={inputRef} value={q} onChange={(e) => setQ(e.target.value)}
                placeholder={searchPlaceholder}
                className="h-8 w-full rounded-sm bg-transparent pl-7 pr-2 text-sm outline-none placeholder:text-muted-foreground"
              />
            </div>
            <div className="max-h-60 overflow-auto">
              {matches.length === 0 ? (
                <div className="px-2 py-4 text-center text-sm text-muted-foreground">{emptyText}</div>
              ) : matches.map((o) => (
                <button
                  key={o.value} type="button"
                  onClick={() => { onChange(o.value); setOpen(false) }}
                  className="flex w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground"
                >
                  <span className="min-w-0">
                    <span className="truncate">{o.label}</span>
                    {o.hint && <span className="ml-1.5 text-xs text-muted-foreground">{o.hint}</span>}
                  </span>
                  {o.value === value && <Check className="h-4 w-4 shrink-0 text-primary" />}
                </button>
              ))}
            </div>
          </div>
        </>
      )}
    </div>
  )
}
