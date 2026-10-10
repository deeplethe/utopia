/** Read the comparisons recorded by regular alignment; refreshing never runs a model. */
import { Fragment, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ChevronDown, ChevronRight, ExternalLink, RefreshCw } from "lucide-react";
import { api, type OntologyRegressionCase } from "../api";
import { S } from "../i18n";
import { fmtObjectValue } from "../objectValue";
import {
  Button,
  IconButton,
  Loading,
  PageHeader,
  Status,
  Table,
  TBody,
  Td,
  Th,
  THead,
  Tr,
  localDateTime,
} from "../ui";

const directionText = (direction: "forward" | "reverse") =>
  direction === "reverse" ? S.ontology.caseReverse : S.ontology.caseForward;

export function OntologyRegressionsPanel({ kbId }: { kbId: string }) {
  const cases = useQuery({
    queryKey: ["ontologyRegressions", kbId],
    queryFn: () => api.ontologyRegressions(kbId),
  });
  return (
    <div>
      <PageHeader
        title={S.ontology.casesTitle}
        sub={S.ontology.casesHint}
        actions={
          <Button
            size="sm"
            disabled={cases.isFetching}
            onClick={() => cases.refetch()}
          >
            <RefreshCw size={14} />
            {S.nav.refresh}
          </Button>
        }
      />
      {cases.isError ? (
        <p role="alert" className="text-body text-danger">
          {(cases.error as Error).message}
        </p>
      ) : cases.isPending ? (
        <Loading>{S.nav.loading}</Loading>
      ) : cases.data.cases.length === 0 ? (
        <p className="text-body text-ink-2">{S.ontology.casesEmpty}</p>
      ) : (
        <Table>
          <THead>
            <Tr>
              <Th>{S.ontology.caseStatement}</Th>
              <Th>{S.ontology.caseExpected}</Th>
              <Th>{S.ontology.caseActual}</Th>
              <Th>{S.ontology.caseResult}</Th>
              <Th>{S.ontology.caseChecked}</Th>
              <Th>{S.ontology.caseOrigin}</Th>
            </Tr>
          </THead>
          <TBody>
            {cases.data.cases.map((item) => (
              <CaseRow key={item.id} kbId={kbId} item={item} />
            ))}
          </TBody>
        </Table>
      )}
    </div>
  );
}

function CaseRow({ kbId, item }: { kbId: string; item: OntologyRegressionCase }) {
  const [open, setOpen] = useState(false);
  const result = item.last_result;
  return (
    <Fragment>
      <Tr>
        <Td>
          <div className="flex items-start gap-2">
            <IconButton
              size="sm"
              label={open ? S.ontology.caseHideEvidence : S.ontology.caseEvidence}
              aria-expanded={open}
              onClick={() => setOpen(!open)}
            >
              {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </IconButton>
            <div className="min-w-0 break-words">
              <span>{item.subject_label}</span>{" "}
              <span className="text-ink-2">{item.phrase}</span>{" "}
              <span>{item.object_label ?? fmtObjectValue(item.object_value) ?? "—"}</span>
            </div>
          </div>
        </Td>
        <Td>
          <div>{item.expected_property_label}</div>
          <div className="text-fine text-ink-2">{directionText(item.expected_direction)}</div>
        </Td>
        <Td>
          <div>
            {item.actual_property_label ??
              (result?.status === "bound" ? S.ontology.caseUnavailableProperty : result?.status === "none" ? S.ontology.caseUnbound : result ? S.ontology.caseUndecided : "—")}
          </div>
          {result?.actual_direction && (
            <div className="text-fine text-ink-2">{directionText(result.actual_direction)}</div>
          )}
          {result?.decided_at && (
            <div className="mt-1 text-fine text-ink-2">
              {S.ontology.caseDecidedAt}: <span className="u-num">{localDateTime(result.decided_at)}</span>
            </div>
          )}
        </Td>
        <Td>
          <Status tone={!result ? "neutral" : result.passed ? "success" : "warn"}>
            {!result ? S.ontology.caseNotChecked : result.human_bound ? S.ontology.caseHumanBound : result.passed ? S.ontology.casePassed : S.ontology.caseMismatch}
          </Status>
          {result?.human_bound && (
            <p className="mt-1 max-w-64 text-fine text-ink-2">{S.ontology.caseHumanHint}</p>
          )}
        </Td>
        <Td className="text-small u-num">
          {item.last_checked_at ? localDateTime(item.last_checked_at) : "—"}
        </Td>
        <Td>
          <div className="text-small">
            {item.origin === "adoption" ? S.ontology.caseAdoption : S.ontology.casePerson}
          </div>
          <div className="text-fine text-ink-2">{item.created_by_label ?? S.ontology.caseUnknownCreator}</div>
          <div className="text-fine text-ink-2 u-num">{localDateTime(item.created_at)}</div>
        </Td>
      </Tr>
      {open && (
        <Tr>
          <Td colSpan={6}>
            <CaseEvidence kbId={kbId} statementId={item.statement_id} />
          </Td>
        </Tr>
      )}
    </Fragment>
  );
}

function CaseEvidence({ kbId, statementId }: { kbId: string; statementId: string }) {
  const evidence = useQuery({
    queryKey: ["evidence", statementId],
    queryFn: () => api.factEvidence(kbId, statementId),
  });
  if (evidence.isError) return <p role="alert" className="text-small text-danger">{(evidence.error as Error).message}</p>;
  if (evidence.isPending) return <Loading>{S.nav.loading}</Loading>;
  if (evidence.data.evidence.length === 0) return <p className="text-small text-ink-2">{S.graph.noEvidence}</p>;
  return (
    <div className="space-y-3">
      {evidence.data.evidence.map((ev) => (
        <div key={ev.chunk_id} className="text-small">
          <p className="whitespace-pre-wrap text-ink">{ev.quote ? `“${ev.quote}”` : S.graph.noQuote}</p>
          <div className="mt-1 flex items-center gap-2 text-fine text-ink-2">
            {ev.document_deleted ? (
              <span>{ev.filename} · {S.graph.sourceDeleted}</span>
            ) : (
              <Link
                to="/kb/$kbId/doc/$docId"
                params={{ kbId, docId: ev.document_id }}
                search={{ chunk: ev.chunk_id }}
                className="u-hover-ink inline-flex items-center gap-1"
              >
                {S.graph.sectionRef(ev.filename, ev.seq + 1)}
                <ExternalLink size={11} />
              </Link>
            )}
            {ev.stale && <span>{S.graph.fromVersion(ev.doc_version)}</span>}
          </div>
        </div>
      ))}
    </div>
  );
}
