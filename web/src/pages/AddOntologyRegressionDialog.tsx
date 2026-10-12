/** A person writes the expectation; the current model binding is never preselected. */
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import { Field, FormDialog, Loading, SearchSelect, Segmented } from "../ui";

export function AddOntologyRegressionDialog({
  kbId, statementId, statement, isValue, onClose,
}: {
  kbId: string;
  statementId: string;
  statement: string;
  isValue: boolean;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const ontology = useQuery({
    queryKey: ["ontology", kbId],
    queryFn: () => api.ontology(kbId),
  });
  const [property, setProperty] = useState("");
  const [direction, setDirection] = useState<"forward" | "reverse">("forward");
  const options = (ontology.data?.relation_types ?? [])
    .filter((p) => p.kind === (isValue ? "attribute" : "relation"))
    .map((p) => ({ value: p.id, label: p.label, hint: p.key }));
  const add = useMutation({
    mutationFn: () => api.addOntologyRegression(kbId, {
      statement_id: statementId,
      relation_type_id: property,
      direction: isValue ? "forward" : direction,
    }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["ontologyRegressions", kbId] });
      toast.success(S.ontology.caseAdded);
      onClose();
    },
    onError: (e) => toast.error((e as Error).message),
  });
  return (
    <FormDialog
      title={S.ontology.caseAddTitle}
      description={S.ontology.caseAddHint}
      closeLabel={S.ui.close}
      saveLabel={S.ontology.caseAdd}
      cancelLabel={S.ontology.cancel}
      canSave={options.some((p) => p.value === property)}
      busy={add.isPending}
      onSave={() => add.mutate()}
      onCancel={onClose}
    >
      <p className="mb-4 text-body text-ink">{statement}</p>
      {ontology.isError ? (
        <p role="alert" className="text-small text-danger">{(ontology.error as Error).message}</p>
      ) : ontology.isPending ? (
        <Loading>{S.nav.loading}</Loading>
      ) : options.length === 0 ? (
        <p className="text-small text-ink-2">{S.ontology.caseNoProperties}</p>
      ) : (
        <div className="space-y-4">
          <Field label={S.ontology.caseExpectedProperty}>
            <SearchSelect value={property} options={options} onChange={setProperty} />
          </Field>
          <Field label={S.ontology.caseDirection}>
            <Segmented
              value={direction}
              onChange={setDirection}
              options={[
                { value: "forward" as const, label: S.ontology.caseForward },
                ...(!isValue ? [{ value: "reverse" as const, label: S.ontology.caseReverse }] : []),
              ]}
            />
          </Field>
        </div>
      )}
    </FormDialog>
  );
}
