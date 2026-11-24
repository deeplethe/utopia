import type { Artifact, Release } from "@neverblink/ohi";

/** 与服务端 `MAX_ONTOLOGY_BYTES`（ontology_routes.rs）同一个数。路由层放到 16 MB，
 *  可 read_upload 在 8 MB 就拒了——在浏览器里先比一次，不白下一个注定被退回的文件 */
export const ONTOLOGY_UPLOAD_LIMIT = 8 * 1024 * 1024;

/** 导入器读得了的两种（`RdfFormat::detect`）。rdfs-turtle 也读得了，但它是 ohi
 *  从 OWL 降出来的，丢了 functional、disjointWith 这些公理，没理由挑它 */
const EXTENSION: Partial<Record<Artifact["format"], string>> = {
  "owl-turtle": "ttl",
  "owl-rdfxml": "rdf",
};

/** 挑一个文件来导：**先挑发布者自己写的那份**（UPSTREAM），其次 Turtle。
 *  0001 判据 1 是原文保真：存进 blob 的应当是词表作者的原文，不是 ohi 转写的那份。
 *  放不进上传上限的不挑；一份都放不进时报最小的那个尺寸，让人知道差多少 */
export function pickArtifact(
  artifacts: Artifact[],
): { artifact: Artifact } | { tooLarge: number } | null {
  const readable = artifacts.filter((a) => EXTENSION[a.format]);
  if (readable.length === 0) return null;
  const fits = readable
    .filter((a) => a.size_bytes <= ONTOLOGY_UPLOAD_LIMIT)
    .sort(
      (a, b) =>
        Number(b.origin === "UPSTREAM") - Number(a.origin === "UPSTREAM") ||
        Number(b.format === "owl-turtle") - Number(a.format === "owl-turtle"),
    );
  if (fits.length > 0) return { artifact: fits[0] };
  return { tooLarge: Math.min(...readable.map((a) => a.size_bytes)) };
}

/** 导入历史只记文件名，所以名字里要带上是哪一版：`w3c-sosa@2017.ttl`。
 *  后缀决定导入器按哪种语法读，必须跟着格式走，不能跟着 URL 走 */
export function artifactFilename(release: Release, artifact: Artifact): string {
  const stem = `${release.namespace}-${release.name}@${release.version}`;
  return `${stem.replace(/[^A-Za-z0-9._@-]/g, "_")}.${EXTENSION[artifact.format]}`;
}
