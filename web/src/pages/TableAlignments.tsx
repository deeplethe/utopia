import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Database } from "lucide-react";
import { api, type TableAlignmentProposal } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import { Button, CARD_ACTIONS, Dropdown, EmptyState, ErrorText, Loading, Status, Table, Th, Td } from "../ui";
import { expressionText } from "./ruleExpressions";

export function TableAlignments({ kbId }: { kbId: string }) {
  const client = useQueryClient();
  const [status, setStatus] = useState("open");
  const detail = useQuery({ queryKey: ["kb", kbId], queryFn: () => api.kbDetail(kbId) });
  const role = detail.data?.my_role;
  const canEdit = role === "editor" || role === "admin" || role === "owner";
  const data = useQuery({ queryKey: ["table-alignments", kbId], queryFn: () => api.tableAlignments(kbId) });
  const decide = useMutation({
    mutationFn: ({ proposal, status }: { proposal: TableAlignmentProposal; status: "adopted" | "rejected" }) =>
      api.decideTableAlignment(kbId, proposal, status),
    onError: (error: Error) => toast.error(error.message),
    onSettled: () => {
      client.invalidateQueries({ queryKey: ["table-alignments", kbId] });
      client.invalidateQueries({ queryKey: ["ontology", kbId] });
    },
  });
  const items = data.data?.items.filter((p) => status === "all" || p.status === status) ?? [];
  const run = data.data?.last_run;
  return <div className="space-y-4">
    <p className="text-small text-ink-2">{S.mapping.alignmentHint}</p>
    <Dropdown className="w-40" value={status} onChange={setStatus} options={[
      { value: "open", label: S.mapping.filterProposed },
      { value: "adopted", label: S.mapping.alignmentAdopted },
      { value: "rejected", label: S.mapping.filterRejected },
      { value: "all", label: S.mapping.filterAll },
    ]} />
    {run && <p className="text-small text-ink-2">{S.mapping.lastRun({
      tables: run.tables_scanned, columns: run.columns_scanned,
      returned: run.returned, accepted: run.accepted, truncated: run.schema_truncated,
    })}</p>}
    {data.isError ? <ErrorText>{data.error.message}</ErrorText> : data.isPending ? <Loading>{S.nav.loading}</Loading> :
      items.length === 0 ? <EmptyState icon={<Database size={20} />}>{S.mapping.alignmentEmpty}</EmptyState> :
      items.map((p) => {
        const d = p.payload.draft;
        const attributes = Object.entries(p.payload.attribute_ids).map(([key, id]) => ({
          id, key, label: key,
        }));
        const declarations = [...d.attribute_types, ...d.relation_types];
        return <section key={p.id} className="glass rounded-panel p-4 space-y-4">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h2 className="text-title font-medium break-words">{d.table} → {d.class}</h2>
              <p className="text-small text-ink-2">{p.payload.source}</p>
            </div>
            <Status tone={p.status === "open" ? "warn" : p.status === "adopted" ? "success" : "neutral"}>
              {p.status === "open" ? S.mapping.filterProposed : p.status === "adopted" ? S.mapping.alignmentAdopted : S.mapping.filterRejected}
            </Status>
          </div>
          <p className="text-body">{d.summary}</p>
          <div className="overflow-x-auto">
            <Table>
              <thead><tr><Th>{S.mapping.alignmentColumn}</Th><Th>{S.mapping.alignmentOwner}</Th><Th>{S.mapping.alignmentProperty}</Th><Th>{S.mapping.alignmentConversion}</Th></tr></thead>
              <tbody>{d.columns.map((c) => <tr key={c.column}>
                <Td><span className="font-mono text-small">{c.column}</span></Td>
                <Td>{c.class}</Td><Td>{c.property}</Td>
                <Td><span className="font-mono text-small break-words">{c.target_class ? `→ ${c.target_class}` : expressionText(c.expression, attributes, S.mapping.noDefinition)}</span></Td>
              </tr>)}</tbody>
            </Table>
          </div>
          {d.omitted.length > 0 && <div>
            <h3 className="text-small font-medium mb-2">{S.mapping.alignmentOmitted}</h3>
            <dl className="text-small text-ink-2 space-y-1">{d.omitted.map((c) => <div key={c.column} className="flex flex-wrap gap-2"><dt className="font-mono">{c.column}</dt><dd>{c.reason}</dd></div>)}</dl>
          </div>}
          {(d.entity_types.length > 0 || declarations.length > 0) && <div>
            <h3 className="text-small font-medium mb-2">{S.mapping.alignmentNewVocabulary}</h3>
            <ul className="space-y-2 text-small">{d.entity_types.map((c) => <li key={`class:${c.key}`}>
              <span className="font-medium">{c.label}</span> <span className="font-mono">({c.key})</span>{c.parents.length > 0 && ` → ${c.parents.join(", ")}`} — {c.description}
            </li>)}{declarations.map((p) => <li key={`property:${p.key}`}>
              <span className="font-medium">{p.label}</span> <span className="font-mono">({p.key})</span> — {p.domains.join(", ")} → {p.datatype ?? p.ranges.join(", ")}{p.unit && ` (${p.unit})`}: {p.description}
            </li>)}</ul>
          </div>}
          {p.status === "open" && canEdit && <div className={CARD_ACTIONS}>
            <Button size="sm" variant="secondary" disabled={decide.isPending} onClick={() => decide.mutate({ proposal: p, status: "adopted" })}>{S.mapping.alignmentAdopt}</Button>
            <Button size="sm" variant="ghost" className="text-danger" disabled={decide.isPending} onClick={() => decide.mutate({ proposal: p, status: "rejected" })}>{S.mapping.alignmentReject}</Button>
          </div>}
        </section>;
      })}
  </div>;
}
