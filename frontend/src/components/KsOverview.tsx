import type { ReactNode } from "react"
import { useMemo } from "react"
import { Bar, BarChart, CartesianGrid, LabelList, Pie, PieChart, XAxis } from "recharts"
import { GitBranch } from "lucide-react"
import type { Conflict, KnowledgeSystem, OntologyView, SourceDoc } from "@/lib/types"
import { Badge } from "@/components/ui/badge"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import {
  type ChartConfig,
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
} from "@/components/ui/chart"

const compositionConfig = {
  value: { label: "Count" },
  classes: { label: "Classes", color: "var(--chart-1)" },
  object: { label: "Object properties", color: "var(--chart-2)" },
  data: { label: "Data properties", color: "var(--chart-3)" },
} satisfies ChartConfig

const axiomConfig = { count: { label: "Axioms", color: "var(--chart-2)" } } satisfies ChartConfig

function Info({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-3">
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="min-w-0 truncate text-right">{children}</span>
    </div>
  )
}

export default function KsOverview({
  ks, view, sources, conflicts,
}: {
  ks: KnowledgeSystem
  view: OntologyView
  sources: SourceDoc[]
  conflicts: Conflict[]
}) {
  const composition = useMemo(
    () =>
      [
        { key: "classes", value: view.classes.length, fill: "var(--color-classes)" },
        { key: "object", value: view.object_properties.length, fill: "var(--color-object)" },
        { key: "data", value: view.data_properties.length, fill: "var(--color-data)" },
      ].filter((d) => d.value > 0),
    [view],
  )

  const axioms = useMemo(
    () => [
      { type: "Subclass", count: view.axioms.subclass_of.length },
      { type: "Disjoint", count: view.axioms.disjoint_with.length },
      { type: "Equivalent", count: view.axioms.equivalent_class.length },
    ],
    [view],
  )

  const topDocs = useMemo(
    () => [...sources].sort((a, b) => b.axiom_count - a.axiom_count).slice(0, 5),
    [sources],
  )

  if (view.classes.length === 0 && view.object_properties.length === 0) {
    return (
      <div className="flex h-48 flex-col items-center justify-center gap-2 rounded-lg border border-dashed text-sm text-muted-foreground">
        <GitBranch className="h-6 w-6" />
        No statistics yet. Use "Extract from Documents" to build the ontology first.
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {/* Two small charts */}
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="pb-0"><CardTitle className="text-sm">Composition</CardTitle></CardHeader>
          <CardContent>
            <ChartContainer config={compositionConfig} className="mx-auto h-[200px] w-full">
              <PieChart>
                <ChartTooltip content={<ChartTooltipContent nameKey="key" hideLabel />} />
                <Pie
                  data={composition} dataKey="value" nameKey="key"
                  innerRadius={45} outerRadius={78} strokeWidth={2} isAnimationActive={false}
                />
                <ChartLegend content={<ChartLegendContent nameKey="key" />} className="-mt-2 flex-wrap gap-x-3 gap-y-1" />
              </PieChart>
            </ChartContainer>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-0"><CardTitle className="text-sm">Axioms by type</CardTitle></CardHeader>
          <CardContent>
            <ChartContainer config={axiomConfig} className="h-[200px] w-full">
              <BarChart accessibilityLayer data={axioms} margin={{ top: 20 }}>
                <CartesianGrid vertical={false} />
                <XAxis dataKey="type" tickLine={false} axisLine={false} tickMargin={8} />
                <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel />} />
                <Bar dataKey="count" fill="var(--color-count)" radius={6} isAnimationActive={false}>
                  <LabelList dataKey="count" position="top" className="fill-foreground" fontSize={12} />
                </Bar>
              </BarChart>
            </ChartContainer>
          </CardContent>
        </Card>
      </div>

      {/* Other info below */}
      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm">Details</CardTitle></CardHeader>
          <CardContent className="space-y-2 text-sm">
            <Info label="Description">
              {ks.description || <span className="text-muted-foreground">No description</span>}
            </Info>
            <Info label="Source documents">{sources.length}</Info>
            <Info label="Open conflicts">
              {conflicts.length > 0
                ? <Badge variant="outline" className="text-[10px]">{conflicts.length}</Badge>
                : <span className="text-muted-foreground">0</span>}
            </Info>
            <Info label="Created">{new Date(ks.created_at).toLocaleString()}</Info>
            <Info label="Updated">{new Date(ks.updated_at).toLocaleString()}</Info>
            <Info label="Base IRI">
              <span className="font-mono text-xs text-muted-foreground">{ks.base_iri}</span>
            </Info>
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2"><CardTitle className="text-sm">Top source documents</CardTitle></CardHeader>
          <CardContent>
            {topDocs.length === 0 ? (
              <p className="py-4 text-sm text-muted-foreground">No source documents yet.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Document</TableHead>
                    <TableHead className="text-right">Chunks</TableHead>
                    <TableHead className="text-right">Axioms</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {topDocs.map((d) => (
                    <TableRow key={d.document_id}>
                      <TableCell className="max-w-[200px] truncate font-medium">{d.filename}</TableCell>
                      <TableCell className="text-right text-muted-foreground">{d.chunk_count}</TableCell>
                      <TableCell className="text-right text-muted-foreground">{d.axiom_count}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
