import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Search } from "lucide-react";
import {
  CatalogClient,
  CatalogException,
  ohiReleasePage,
  type SearchHit,
} from "@neverblink/ohi";
import { S } from "../i18n";
import { Button, Input, Table, TBody, Td, Th, THead, Tr } from "../ui";
import { artifactFilename, pickArtifact } from "./ohiCatalog";

/* 从 ohi.dev（公开的本体目录）找一份词表，交给上面同一条「预览 → 确认」。
   #314、#923 里人要自己去找文件、自己把几百个模块并成一份；ohi 上 FIBO 已经是
   并好的一个文件。**服务端一个字节都不碰外网**：查询与下载都由浏览器直接发给
   ohi.dev，所以离线部署不受影响——内网里搜索只会说连不上，选文件照旧。
   打开页面时不发请求，只有人按了搜索，查询才离开这台机器 */

const catalog = new CatalogClient();

/** SDK 的报错是英文的传输细节；连不上与超时对人来说是同一件事 */
function catalogError(e: unknown): unknown {
  if (
    e instanceof CatalogException &&
    (e.error.code === "CONNECTION_ERROR" || e.error.code === "TIMEOUT")
  ) {
    return new Error(S.ontology.ohiUnreachable);
  }
  return e;
}

export function OhiCatalogSearch({
  disabled,
  onPick,
  onError,
}: {
  disabled: boolean;
  onPick: (f: File) => void;
  onError: (e: unknown) => void;
}) {
  const [query, setQuery] = useState("");

  const search = useMutation({
    mutationFn: (q: string) => catalog.search({ q, per_page: 10 }),
    onError: (e) => onError(catalogError(e)),
  });

  const download = useMutation({
    mutationFn: async (hit: SearchHit) => {
      const publication = await catalog.inspect(hit.base_url);
      const picked = pickArtifact(publication.artifacts);
      if (picked === null) throw new Error(S.ontology.ohiNoOwl);
      if ("tooLarge" in picked) {
        throw new Error(S.ontology.ohiTooLarge(picked.tooLarge));
      }
      const bytes = await catalog.get(picked.artifact.url);
      // SDK 声明的是 Uint8Array<ArrayBufferLike>，File 只收 ArrayBuffer 背的那种；
      // 拷一份（至多 8 MB）比断言类型诚实
      return new File(
        [new Uint8Array(bytes)],
        artifactFilename(publication, picked.artifact),
      );
    },
    onSuccess: (f) => {
      search.reset();
      onPick(f);
    },
    onError: (e) => onError(catalogError(e)),
  });

  const result = search.data;
  const busy = disabled || search.isPending || download.isPending;

  return (
    <div className="mt-4">
      <p className="text-small text-ink-2">{S.ontology.ohiHint}</p>
      <form
        className="mt-2 flex items-center gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (query.trim()) search.mutate(query.trim());
        }}
      >
        <Input
          icon={<Search size={13} />}
          className="w-64"
          placeholder={S.ontology.ohiPlaceholder}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button size="sm" variant="secondary" type="submit" disabled={busy}>
          {search.isPending ? S.ontology.ohiSearching : S.ontology.ohiSearch}
        </Button>
        {download.isPending && (
          <span className="text-small text-ink-2">
            {S.ontology.ohiDownloading}
          </span>
        )}
      </form>

      {result && result.hits.length === 0 && (
        <p className="mt-2 text-body text-ink-2">{S.ontology.ohiNoMatch}</p>
      )}
      {result && result.hits.length > 0 && (
        <div className="mt-2">
          <Table>
            <THead>
              <Tr>
                <Th>{S.ontology.ohiOntology}</Th>
                <Th>{S.ontology.ohiPublisher}</Th>
                <Th>{S.ontology.ohiLicense}</Th>
                <Th />
              </Tr>
            </THead>
            <TBody>
              {result.hits.map((hit) => (
                <Tr key={hit.base_url}>
                  <Td>
                    <div>{hit.title}</div>
                    <a
                      href={ohiReleasePage(hit)}
                      target="_blank"
                      rel="noreferrer"
                      className="u-link font-mono text-fine"
                    >
                      {hit.namespace}/{hit.name}@{hit.version}
                    </a>
                  </Td>
                  <Td className="text-small text-ink-2">
                    {hit.publishers.map((p) => p.name).join(", ")}
                  </Td>
                  <Td className="text-small text-ink-2">
                    {hit.licenses.join(", ")}
                  </Td>
                  <Td className="text-right">
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={busy}
                      onClick={() => download.mutate(hit)}
                    >
                      {S.ontology.ohiPreview}
                    </Button>
                  </Td>
                </Tr>
              ))}
            </TBody>
          </Table>
          {result.total > result.hits.length && (
            <p className="mt-2 text-fine text-ink-2">
              {S.ontology.ohiShown(result.hits.length, result.total)}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
