import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react"
import { AlertTriangle } from "lucide-react"

import { useI18n } from "@/lib/i18n"
import { Button } from "@/components/ui/button"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"

type ConfirmOptions = {
  title?: string
  confirmLabel?: string
  cancelLabel?: string
  destructive?: boolean
}

type ConfirmRequest = ConfirmOptions & { message: string }
type ConfirmAction = (message: string, options?: ConfirmOptions) => Promise<boolean>

const ConfirmContext = createContext<ConfirmAction | null>(null)

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const { t } = useI18n()
  const [request, setRequest] = useState<ConfirmRequest | null>(null)
  const resolver = useRef<((confirmed: boolean) => void) | null>(null)

  const settle = useCallback((confirmed: boolean) => {
    resolver.current?.(confirmed)
    resolver.current = null
    setRequest(null)
  }, [])

  const confirmAction = useCallback<ConfirmAction>((message, options = {}) => new Promise((resolve) => {
    resolver.current?.(false)
    resolver.current = resolve
    setRequest({ message, ...options })
  }), [])

  useEffect(() => () => resolver.current?.(false), [])

  return (
    <ConfirmContext.Provider value={confirmAction}>
      {children}
      <Dialog open={request !== null} onOpenChange={(open) => { if (!open) settle(false) }}>
        <DialogContent className="border bg-background shadow-2xl sm:max-w-md" showCloseButton={false}>
          <DialogHeader>
            <div className="mb-1 flex h-9 w-9 items-center justify-center rounded-full bg-muted">
              <AlertTriangle className={`h-4 w-4 ${request?.destructive ? "text-destructive" : "text-foreground"}`} />
            </div>
            <DialogTitle>{request?.title ?? t("confirm.title")}</DialogTitle>
            <DialogDescription className="whitespace-pre-wrap leading-relaxed">{request?.message}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => settle(false)}>{request?.cancelLabel ?? t("common.cancel")}</Button>
            <Button variant={request?.destructive ? "destructive" : "default"} onClick={() => settle(true)}>
              {request?.confirmLabel ?? t("confirm.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </ConfirmContext.Provider>
  )
}

export function useConfirm() {
  const confirmAction = useContext(ConfirmContext)
  if (!confirmAction) throw new Error("useConfirm must be used inside ConfirmProvider")
  return confirmAction
}
