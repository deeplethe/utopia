import { SlidersHorizontal } from "lucide-react"
import { Link, useSearchParams } from "react-router-dom"

import { useI18n } from "@/lib/i18n"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import ExtractionQueuePanel from "@/components/ExtractionQueuePanel"
import KsDocuments from "@/components/KsDocuments"

export default function DocumentsExtractionPanel({
  ksId,
  canWrite,
  onChanged,
}: {
  ksId: number
  canWrite: boolean
  onChanged?: () => void
}) {
  const { t } = useI18n()
  const [searchParams, setSearchParams] = useSearchParams()
  const tab = searchParams.get("tab") === "queue" ? "queue" : "documents"

  const changeTab = (value: string) => {
    const next = new URLSearchParams(searchParams)
    if (value === "queue") next.set("tab", "queue")
    else next.delete("tab")
    setSearchParams(next, { replace: true })
  }

  return (
    <Tabs value={tab} onValueChange={changeTab} className="min-w-0 gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <TabsList>
          <TabsTrigger value="documents">{t("documentsWorkspace.files")}</TabsTrigger>
          <TabsTrigger value="queue">{t("extractionQueue.title")}</TabsTrigger>
        </TabsList>
        <Button asChild size="sm" variant="outline">
          <Link to={`/knowledge/${ksId}/prompts`}>
            <SlidersHorizontal className="h-4 w-4" />
            {t("documentsWorkspace.promptSettings")}
          </Link>
        </Button>
      </div>

      <TabsContent value="documents" className="min-w-0">
        <KsDocuments ksId={ksId} canWrite={canWrite} onChanged={onChanged} />
      </TabsContent>
      <TabsContent value="queue" className="min-w-0">
        <ExtractionQueuePanel ksId={ksId} showTitle={false} />
      </TabsContent>
    </Tabs>
  )
}
