// 本体的编辑弹窗：类、关系、属性，以及「用一个已有的关系连接这个类」。
//
// 右侧面板只展示，改动开一扇窗——从前三张表单嵌在面板里，改到一半的字段
// 跟正在读的定义挤在同一列，保存与删除也挨在一起。字段一个没变，搬的是壳：
// 表单里的每一项照旧，只是有了自己的标题、自己的页脚和 Esc。
import { useMemo, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { api, type EntityTypeView, type RelationTypeView } from "../api";
import { S } from "../i18n";
import { toast } from "../toast";
import {
  Checkbox,
  ColorPicker,
  colorForKey,
  Dropdown,
  Field,
  FormDialog,
  Input,
  MultiSearchSelect,
  SearchSelect,
  Segmented,
  Textarea,
} from "../ui";

/** 父类下拉的候选：树序 + 缩进（层级可见），排除自己与全部后代（防成环）。 */
export function parentOptions(
  allTypes: EntityTypeView[],
  selfId: string | undefined,
): { value: string; label: string; indent: number }[] {
  const excluded = new Set<string>();
  if (selfId) {
    excluded.add(selfId);
    // 收后代：反复扫描直到收敛（类的数量级很小，O(n²) 无所谓）
    let grew = true;
    while (grew) {
      grew = false;
      for (const t of allTypes) {
        if (t.parents.some((p) => excluded.has(p)) && !excluded.has(t.id)) {
          excluded.add(t.id);
          grew = true;
        }
      }
    }
  }
  const children = new Map<string | null, EntityTypeView[]>();
  for (const t of allTypes) {
    const p = t.primary_parent ?? null;
    if (!children.has(p)) children.set(p, []);
    children.get(p)!.push(t);
  }
  const out: { value: string; label: string; indent: number }[] = [];
  const walk = (parent: string | null, depth: number) => {
    for (const t of children.get(parent) ?? []) {
      if (excluded.has(t.id)) continue;
      out.push({ value: t.id, label: t.label, indent: depth });
      walk(t.id, depth + 1);
    }
  };
  walk(null, 0);
  return out;
}

const toggleIn = (
  set: React.Dispatch<React.SetStateAction<string[]>>,
  id: string,
) => set((v) => (v.includes(id) ? v.filter((x) => x !== id) : [...v, id]));

/* ---------- 类 ---------- */

export function ClassDialog({
  kbId,
  existing,
  parentId,
  allTypes,
  onClose,
  onSaved,
  onDeleted,
  onError,
}: {
  kbId: string;
  existing: EntityTypeView | null;
  /** 新建时的父类（从左栏或定义页的「新建子类」进来时预填）；多父下它是主父 */
  parentId: string | null;
  allTypes: EntityTypeView[];
  onClose: () => void;
  /** 创建成功时携带新 id，编辑成功时为 undefined */
  onSaved: (createdId?: string) => void;
  onDeleted: () => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  // 新建时颜色跟着 key 走（与后端 color_for_key 同一个规则），不是一个固定默认值。
  // 用户当然可以改；但**不改的话，手动建的类和导入建的类配色体系一致**
  const [color, setColor] = useState(
    existing?.color ?? colorForKey(existing?.key ?? ""),
  );
  const [colorTouched, setColorTouched] = useState(Boolean(existing?.color));
  const [shape, setShape] = useState<"circle" | "square">(
    existing?.shape ?? "circle",
  );
  const [parents, setParents] = useState<string[]>(
    existing?.parents ?? (parentId ? [parentId] : []),
  );
  const [description, setDescription] = useState(existing?.description ?? "");
  // 互斥：声明「不可能同时是」。一致性检查据此报不可满足的类（0002）——
  // 一个类继承了两个互斥的祖先，就永远不可能有实例，而它不报错，只是永远空着
  const [disjoint, setDisjoint] = useState<string[]>(existing?.disjoint ?? []);
  const options = useMemo(
    () => parentOptions(allTypes, existing?.id),
    [allTypes, existing?.id],
  );

  const save = useMutation({
    mutationFn: async (): Promise<unknown> =>
      existing
        ? api.updateEntityType(kbId, existing.id, {
            label,
            color,
            shape,
            parents,
            disjoint,
            description,
          })
        : api.createEntityType(kbId, {
            key,
            label,
            color,
            shape,
            parents,
            disjoint,
            description,
          }),
    onSuccess: (res) => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onSaved(existing ? undefined : (res as { id?: string })?.id);
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteEntityType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDeleted();
    },
    onError,
  });

  return (
    <FormDialog
      title={existing ? S.ontology.editClass : S.ontology.newClass}
      description={
        existing ? `${existing.key} · ${S.ontology.usage(existing.usage)}` : undefined
      }
      closeLabel={S.ui.close}
      saveLabel={S.ontology.save}
      cancelLabel={S.ontology.cancel}
      canSave={!!label.trim() && (!!existing || !!key.trim())}
      busy={save.isPending}
      onSave={() => save.mutate()}
      onCancel={onClose}
      danger={
        existing && !existing.builtin
          ? {
              label: S.ontology.delete,
              disabled: existing.usage > 0 || remove.isPending,
              title: existing.usage > 0 ? S.ontology.deleteBlocked : undefined,
              onClick: () => remove.mutate(),
            }
          : undefined
      }
    >
      {/* key 是纯技术标识：已存在时不展示，只在创建时输入 */}
      {!existing && (
        <Field
          label={
            <>
              {S.ontology.key}{" "}
              <span className="text-ink-2">({S.ontology.keyHint})</span>
            </>
          }
        >
          <Input
            autoFocus
            value={key}
            onChange={(e) => {
              setKey(e.target.value);
              // 用户没自己挑过色，就让颜色跟着 key 走——与后端同一个规则
              if (!colorTouched) setColor(colorForKey(e.target.value));
            }}
            className="w-full"
            placeholder="contract"
          />
        </Field>
      )}
      <Field label={S.ontology.label}>
        <Input
          autoFocus={!!existing}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          className="w-full"
        />
      </Field>
      <Field label={S.ontology.shapeColor}>
        <div className="flex items-center gap-2">
          <ColorPicker
            value={color}
            onChange={(c: string) => {
              setColor(c);
              setColorTouched(true);
            }}
            shape={shape}
          />
          {/* 形状：与图谱节点渲染一一对应（circle=四层圆 / square=四层方） */}
          <Segmented
            size="sm"
            value={shape}
            onChange={setShape}
            options={(["circle", "square"] as const).map((sh) => ({
              value: sh,
              title: sh,
              // 图标占一行正文的高（h-4）：Segmented 的高度由内容撑，
              // 光秃秃的 12px 图标会让它比旁边 32 高的色井矮一截
              label: (
                <span className="flex h-4 items-center">
                  <span
                    className={`h-3 w-3 border-[1.5px] border-current ${
                      sh === "circle" ? "rounded-full" : "scale-90"
                    }`}
                  />
                </span>
              ),
            }))}
          />
        </div>
      </Field>
      {/* 多父：subClassOf 可以有多条。左栏按树画，一个类只能出现一次——
          所以第一个当主父。界面说明这条，不另加一个"选主父"的控件 */}
      <Field
        label={S.ontology.parent}
        hint={parents.length > 1 ? S.ontology.primaryParentHint : undefined}
      >
        <MultiSearchSelect
          values={parents}
          options={options}
          onToggle={(id) => toggleIn(setParents, id)}
          placeholder={S.ontology.searchTypes}
          emptyHint={S.ontology.noParent}
        />
      </Field>
      {/* 互斥：**声明「不可能同时是」**。紧挨着父类，因为两者是同一件事的
          两面——父类说「也是」，互斥说「不可能同时是」，而一致性检查正是
          在这两者打架时报出「这个类永远不可能有实例」。跟自己的父类互斥当场说，
          比让人跑一遍一致性检查再发现要快 */}
      <Field
        label={S.ontology.disjoint}
        hint={S.ontology.disjointHint}
        error={
          disjoint.some((d) => parents.includes(d))
            ? S.ontology.disjointWithParent
            : undefined
        }
      >
        <MultiSearchSelect
          values={disjoint}
          options={options}
          onToggle={(id) => toggleIn(setDisjoint, id)}
          placeholder={S.ontology.searchTypes}
          emptyHint={S.ontology.noDisjoint}
        />
      </Field>
      {/* 语义指引：整段注入抽取 prompt，直接影响抽取归类质量 */}
      <Field
        label={S.ontology.description}
        hint={S.ontology.descriptionHint}
        className="mb-0"
      >
        <Textarea
          className="w-full min-h-28 resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

/* ---------- 关系 ---------- */

export function PropertyDialog({
  kbId,
  existing,
  allTypes,
  allRelations,
  initialDomain,
  onClose,
  onSaved,
  onDeleted,
  onError,
}: {
  kbId: string;
  existing: RelationTypeView | null;
  allTypes: EntityTypeView[];
  /** 本库的关系（不含属性）。逆与子属性的下拉从这里取——**属性不在其中**，
   *  它的宾语是字面值，反过来无从谈起 */
  allRelations: RelationTypeView[];
  /** 从一个类出发新建关系时，domain 预填成那个类——**只影响初始值**，
   *  不是约束：下面照旧是个可编辑的 MultiSearchSelect，填错了随手改 */
  initialDomain: string | null;
  onClose: () => void;
  onSaved: (createdId?: string) => void;
  onDeleted: () => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  const [temporal, setTemporal] = useState(existing?.temporal ?? "state");
  const [functional, setFunctional] = useState(existing?.functional ?? false);
  const [inverseFunctional, setInverseFunctional] = useState(
    existing?.inverse_functional ?? false,
  );
  // 其余四条 OWL 公理。**推理机的判据全在这里**——从前只能靠导入 OWL 带进来，
  // 在界面上手工建本体的人永远开不了那台机器（0002）
  const [transitive, setTransitive] = useState(existing?.is_transitive ?? false);
  const [symmetric, setSymmetric] = useState(existing?.is_symmetric ?? false);
  const [asymmetric, setAsymmetric] = useState(existing?.is_asymmetric ?? false);
  const [irreflexive, setIrreflexive] = useState(
    existing?.is_irreflexive ?? false,
  );
  // 同一族的后两条，形状不同：指向另一个关系。空串 = 没声明
  const [inverseOf, setInverseOf] = useState(existing?.inverse_of ?? "");
  const [subPropertyOf, setSubPropertyOf] = useState(
    existing?.sub_property_of ?? "",
  );
  const [description, setDescription] = useState(existing?.description ?? "");
  const [domains, setDomains] = useState<string[]>(
    existing?.domains ?? (initialDomain ? [initialDomain] : []),
  );
  const [ranges, setRanges] = useState<string[]>(existing?.ranges ?? []);
  // 边上的属性（0037）：这条关系的边能带哪些属性，选的是本库里 kind=attribute 的定义
  const [qualifiers, setQualifiers] = useState<string[]>(existing?.qualifiers ?? []);
  // 显示标签，不显示 key。**进提示词的 key 由服务端从库里取**，与界面显示什么无关
  const typeOpts = useMemo(() => parentOptions(allTypes, undefined), [allTypes]);
  // 两个下拉的选项：本库的其它关系。**自己不进列表**，两条都是：子属性指向自己
  // 数据库直接拒（那是个环）；逆指向自己等于 symmetric，那个复选框就在上面。
  // 唯一的例外是**当前值就是自己**：OWL 导入进来的可以长这样，从列表里漏掉它
  // 就会让下拉显示空白，而空白一保存就把已声明的抹了
  const linkOptions = (current: string) => [
    { value: "", label: S.ontology.noLink },
    ...(existing && current === existing.id
      ? [{ value: existing.id, label: existing.label, hint: existing.key }]
      : []),
    ...allRelations
      .filter((r) => r.id !== existing?.id)
      .map((r) => ({ value: r.id, label: r.label, hint: r.key })),
  ];
  /** 下拉里选中那条的显示名。找不到就回落到 id——宁可难看，不要空着 */
  const nameOf = (id: string) =>
    allRelations.find((r) => r.id === id)?.label ?? id;

  const save = useMutation({
    mutationFn: async (): Promise<unknown> => {
      const body = {
        label,
        temporal,
        functional,
        inverse_functional: inverseFunctional,
        is_transitive: transitive,
        is_symmetric: symmetric,
        is_asymmetric: asymmetric,
        is_irreflexive: irreflexive,
        // 空串要变成 null 再送——服务端收 `Option<Uuid>`，
        // `""` 解不成 UUID，会是一个 422 而不是「清空」
        inverse_of: inverseOf || null,
        sub_property_of: subPropertyOf || null,
        description,
        domains,
        ranges,
        qualifiers,
      };
      return existing
        ? api.updateRelationType(kbId, existing.id, body)
        : api.createRelationType(kbId, { key, ...body });
    },
    onSuccess: (res) => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onSaved(existing ? undefined : (res as { id?: string })?.id);
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteRelationType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDeleted();
    },
    onError,
  });

  return (
    <FormDialog
      width="lg"
      title={existing ? S.ontology.editProperty : S.ontology.newProperty}
      description={
        existing ? `${existing.key} · ${S.ontology.usage(existing.usage)}` : undefined
      }
      closeLabel={S.ui.close}
      saveLabel={S.ontology.save}
      cancelLabel={S.ontology.cancel}
      canSave={!!label.trim() && (!!existing || !!key.trim())}
      busy={save.isPending}
      onSave={() => save.mutate()}
      onCancel={onClose}
      danger={
        existing && !existing.builtin
          ? {
              label: S.ontology.delete,
              disabled: existing.usage > 0 || remove.isPending,
              title: existing.usage > 0 ? S.ontology.deleteBlocked : undefined,
              onClick: () => remove.mutate(),
            }
          : undefined
      }
    >
      {!existing && (
        <Field
          label={
            <>
              {S.ontology.key}{" "}
              <span className="text-ink-2">({S.ontology.keyHint})</span>
            </>
          }
        >
          <Input
            autoFocus
            value={key}
            onChange={(e) => setKey(e.target.value)}
            className="w-full"
            placeholder="signed_with"
          />
        </Field>
      )}
      <Field label={S.ontology.label}>
        <Input
          autoFocus={!!existing}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          className="w-full"
        />
      </Field>
      {/* 类型签名。界面显示标签，而进提示词的是 key —— 那一步在服务端（0004） */}
      <Field label={S.ontology.signature} hint={S.ontology.signatureHint}>
        <div className="grid gap-2 sm:grid-cols-2">
          <div className="min-w-0">
            <div className="mb-1 text-small text-ink-2">{S.ontology.domainLabel}</div>
            <MultiSearchSelect
              values={domains}
              options={typeOpts}
              onToggle={(id) => toggleIn(setDomains, id)}
              placeholder={S.ontology.searchTypes}
              emptyHint={S.ontology.anyType}
            />
          </div>
          <div className="min-w-0">
            <div className="mb-1 text-small text-ink-2">{S.ontology.rangeLabel}</div>
            <MultiSearchSelect
              values={ranges}
              options={typeOpts}
              onToggle={(id) => toggleIn(setRanges, id)}
              placeholder={S.ontology.searchTypes}
              emptyHint={S.ontology.anyType}
            />
          </div>
          <div className="min-w-0">
            {/* 边上的属性（0037）：选项是本库的属性定义，不是类。
                关系不能把自己声明成自己的属性，列表里也不列自己 */}
            <div className="mb-1 text-small text-ink-2">{S.ontology.qualifiers}</div>
            <MultiSearchSelect
              values={qualifiers}
              options={allRelations
                .filter((r) => r.kind === "attribute" && r.id !== existing?.id)
                .map((r) => ({ value: r.id, label: r.label, indent: 0 }))}
              onToggle={(id) => toggleIn(setQualifiers, id)}
              placeholder={S.ontology.searchTypes}
              emptyHint={S.ontology.noQualifiers}
            />
          </div>
        </div>
      </Field>
      <Field label={S.ontology.temporal}>
        <Dropdown
          value={temporal}
          onChange={setTemporal}
          className="w-full"
          options={[
            { value: "state", label: S.ontology.temporalState },
            { value: "event", label: S.ontology.temporalEvent },
            { value: "eternal", label: S.ontology.temporalEternal },
          ]}
        />
      </Field>
      {/* 六条公理并成一组。**它们本来就是同一族**——推理机（0002）拿它们当判据。
          每一条底下写清「勾了会发生什么」：这些开关不是描述，是会改变系统行为的
          声明。对称与反对称同时勾是自相矛盾的（只对空关系成立），当场说一句 */}
      <Field
        label={S.ontology.axioms}
        hint={S.ontology.axiomsHint}
        error={symmetric && asymmetric ? S.ontology.axiomConflict : undefined}
      >
        <div className="space-y-2">
          {(
            [
              [functional, setFunctional, S.ontology.functional, S.ontology.functionalHint],
              [
                inverseFunctional,
                setInverseFunctional,
                S.ontology.inverseFunctional,
                S.ontology.inverseFunctionalHint,
              ],
              [transitive, setTransitive, S.ontology.transitive, S.ontology.transitiveHint],
              [symmetric, setSymmetric, S.ontology.symmetric, S.ontology.symmetricHint],
              [asymmetric, setAsymmetric, S.ontology.asymmetric, S.ontology.asymmetricHint],
              [
                irreflexive,
                setIrreflexive,
                S.ontology.irreflexive,
                S.ontology.irreflexiveHint,
              ],
            ] as const
          ).map(([on, set, title, hint], i) => (
            <Checkbox
              key={i}
              checked={on}
              onChange={(v) => set(v)}
              label={title}
              hint={hint}
            />
          ))}
        </div>
      </Field>
      {/* 同一组的后两条，只是形状不同：它们指向**另一个关系**，所以是下拉不是复选框。
          选了之后当场把话说全——**这两条推出来的事实主宾未必同向**，逆要对调，
          子属性不对调，只看名字分不出来，写出来就分得出 */}
      <div className="grid gap-2 sm:grid-cols-2">
        <Field label={S.ontology.inverseOf} hint={S.ontology.inverseOfHint}>
          <SearchSelect
            value={inverseOf}
            onChange={setInverseOf}
            options={linkOptions(inverseOf)}
            size="sm"
            className="w-full"
            placeholder={S.ontology.noLink}
          />
        </Field>
        <Field label={S.ontology.subPropertyOf} hint={S.ontology.subPropertyOfHint}>
          <SearchSelect
            value={subPropertyOf}
            onChange={setSubPropertyOf}
            options={linkOptions(subPropertyOf)}
            size="sm"
            className="w-full"
            placeholder={S.ontology.noLink}
          />
        </Field>
      </div>
      {(inverseOf || subPropertyOf) && (
        <div className="mb-4 -mt-2 space-y-1 text-fine leading-relaxed text-ink-2">
          {inverseOf && (
            <div>
              {S.ontology.linkMeansInverse(label.trim() || key || "?", nameOf(inverseOf))}
            </div>
          )}
          {subPropertyOf && (
            <div>
              {S.ontology.linkMeansSuper(label.trim() || key || "?", nameOf(subPropertyOf))}
            </div>
          )}
        </div>
      )}
      <Field
        label={S.ontology.description}
        hint={S.ontology.descriptionHint}
        className="mb-0"
      >
        <Textarea
          className="w-full min-h-28 resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

/* ---------- 属性（一个类的字面值字段） ---------- */

export function AttributeDialog({
  kbId,
  typeId,
  allTypes,
  existing,
  onClose,
  onSaved,
  onDeleted,
  onError,
}: {
  kbId: string;
  /** null = 不是从某个类打开的（本体表格的属性页）：弹窗里自己选挂在哪个类 */
  typeId: string | null;
  allTypes: EntityTypeView[];
  existing: RelationTypeView | null;
  onClose: () => void;
  onSaved: () => void;
  onDeleted: () => void;
  onError: (e: unknown) => void;
}) {
  const [key, setKey] = useState(existing?.key ?? "");
  const [label, setLabel] = useState(existing?.label ?? "");
  const [datatype, setDatatype] = useState(existing?.datatype ?? "text");
  const [unit, setUnit] = useState(existing?.unit ?? "");
  // 单值 = functional：新值经时态引擎闭合旧值（属性历史的来源）。多数属性如此，默认开
  const [single, setSingle] = useState(existing?.functional ?? true);
  const [description, setDescription] = useState(existing?.description ?? "");
  const [host, setHost] = useState(typeId ?? "");
  const hostOpts = useMemo(() => parentOptions(allTypes, undefined), [allTypes]);

  const save = useMutation({
    mutationFn: async (): Promise<unknown> =>
      existing
        ? api.updateRelationType(kbId, existing.id, {
            label,
            temporal: existing.temporal,
            functional: single,
            inverse_functional: false,
            description,
            datatype,
            unit,
          })
        : api.createRelationType(kbId, {
            key,
            label,
            kind: "attribute",
            domains: [host],
            temporal: "state",
            functional: single,
            inverse_functional: false,
            description,
            datatype,
            unit,
          }),
    onSuccess: () => {
      toast.success(existing ? S.toast.saved : S.toast.created);
      onSaved();
    },
    onError,
  });
  const remove = useMutation({
    mutationFn: () => api.deleteRelationType(kbId, existing!.id),
    onSuccess: () => {
      toast.success(S.toast.deleted);
      onDeleted();
    },
    onError,
  });

  return (
    <FormDialog
      title={existing ? S.ontology.editAttribute : S.ontology.newAttribute}
      description={
        existing ? `${existing.key} · ${S.ontology.usage(existing.usage)}` : undefined
      }
      closeLabel={S.ui.close}
      saveLabel={S.ontology.save}
      cancelLabel={S.ontology.cancel}
      canSave={!!label.trim() && (!!existing || (!!key.trim() && !!host))}
      busy={save.isPending}
      onSave={() => save.mutate()}
      onCancel={onClose}
      danger={
        existing
          ? {
              label: S.ontology.delete,
              disabled: existing.usage > 0 || remove.isPending,
              title: existing.usage > 0 ? S.ontology.deleteBlocked : undefined,
              onClick: () => remove.mutate(),
            }
          : undefined
      }
    >
      {!existing && typeId === null && (
        <Field label={S.ontology.attributeHost}>
          <SearchSelect
            value={host}
            onChange={setHost}
            options={hostOpts}
            className="w-full"
            placeholder={S.ontology.searchTypes}
          />
        </Field>
      )}
      {existing ? (
        <Field label={S.ontology.label}>
          <Input
            autoFocus
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            className="w-full"
          />
        </Field>
      ) : (
        <div className="grid grid-cols-2 gap-4">
          <Field label={S.ontology.key}>
            <Input
              autoFocus
              value={key}
              onChange={(e) => setKey(e.target.value)}
              className="w-full"
              placeholder="salary"
            />
          </Field>
          <Field label={S.ontology.label}>
            <Input
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              className="w-full"
            />
          </Field>
        </div>
      )}
      <div className="grid grid-cols-2 gap-4">
        <Field label={S.ontology.attrDatatype}>
          <Dropdown
            value={datatype}
            onChange={(v) => setDatatype(v as typeof datatype)}
            className="w-full"
            options={(["text", "number", "date", "bool"] as const).map((d) => ({
              value: d,
              label: S.ontology.datatypeNames[d],
            }))}
          />
        </Field>
        <Field
          label={
            <>
              {S.ontology.attrUnit}{" "}
              <span className="text-ink-2">({S.ontology.attrUnitHint})</span>
            </>
          }
        >
          <Input
            value={unit}
            onChange={(e) => setUnit(e.target.value)}
            className="w-full"
          />
        </Field>
      </div>
      <div className="mb-4">
        <Checkbox
          checked={single}
          onChange={(v) => setSingle(v)}
          label={S.ontology.attrSingle}
        />
      </div>
      <Field label={S.ontology.description} className="mb-0">
        <Textarea
          className="w-full min-h-24 resize-y"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
      </Field>
    </FormDialog>
  );
}

/* ---------- 用一个已有的关系连接这个类 ---------- */

export function ConnectDialog({
  kbId,
  cls,
  relations,
  onClose,
  onConnected,
  onError,
}: {
  kbId: string;
  cls: EntityTypeView;
  /** kind === "relation" 的那些 */
  relations: RelationTypeView[];
  onClose: () => void;
  /** 连完带着那条关系的 id：调用方跳过去，域/值域改没改一眼可见 */
  onConnected: (relationId: string) => void;
  onError: (e: unknown) => void;
}) {
  const [relationId, setRelationId] = useState("");
  const [side, setSide] = useState<"domain" | "range">("domain");
  const connect = useMutation({
    mutationFn: () => {
      const rel = relations.find((r) => r.id === relationId);
      if (!rel) return Promise.reject(new Error("relation not found"));
      // 只加不减：已经连着的那一侧原样保留，另一侧才可能被追加。
      // Set 去重——挑到已经连过的关系时这是个无害的空操作
      const domains =
        side === "domain" ? [...new Set([...rel.domains, cls.id])] : rel.domains;
      const ranges =
        side === "range" ? [...new Set([...rel.ranges, cls.id])] : rel.ranges;
      return api.updateRelationType(kbId, rel.id, {
        label: rel.label,
        temporal: rel.temporal,
        functional: rel.functional,
        inverse_functional: rel.inverse_functional,
        is_transitive: rel.is_transitive,
        is_symmetric: rel.is_symmetric,
        is_asymmetric: rel.is_asymmetric,
        is_irreflexive: rel.is_irreflexive,
        inverse_of: rel.inverse_of,
        sub_property_of: rel.sub_property_of,
        description: rel.description,
        domains,
        ranges,
      });
    },
    onSuccess: () => {
      const rel = relations.find((r) => r.id === relationId);
      toast.success(S.ontology.schemaConnected(rel?.label ?? ""));
      onConnected(relationId);
    },
    onError,
  });
  return (
    <FormDialog
      width="sm"
      title={S.ontology.connectTitle}
      description={S.ontology.schemaConnectHint}
      closeLabel={S.ui.close}
      saveLabel={S.ontology.schemaConnect}
      cancelLabel={S.ontology.cancel}
      canSave={!!relationId}
      busy={connect.isPending}
      onSave={() => connect.mutate()}
      onCancel={onClose}
    >
      <Field label={S.ontology.tabProperties}>
        <SearchSelect
          value={relationId}
          onChange={setRelationId}
          options={relations.map((r) => ({ value: r.id, label: r.label, hint: r.key }))}
          className="w-full"
          placeholder={S.ontology.schemaConnectPlaceholder}
        />
      </Field>
      <Field label={`${S.ontology.schemaConnectAs} ${cls.label}`} className="mb-0">
        <Segmented
          size="sm"
          value={side}
          onChange={setSide}
          options={[
            { value: "domain", label: S.ontology.domainLabel },
            { value: "range", label: S.ontology.rangeLabel },
          ]}
        />
      </Field>
    </FormDialog>
  );
}
