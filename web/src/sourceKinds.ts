/**
 * 来源的种类——**前端这一份清单只在这里写一次**。
 *
 * 后端的那一份是 `crates/utopia-core` 里的 `SourceKind` 枚举，创建时的校验与同步时的
 * 分派都从它出；`utopia-store` 的测试会读这个文件，把两边对一遍——任何一边多一种
 * 少一种，`cargo test` 就红。此前两边各自手写，五种连接器加了同步分支、进了界面，
 * 却没进创建的白名单：选得到、建不出来（#247）。
 *
 * 顺序就是建来源对话框里的顺序。
 */
export const CREATABLE_SOURCE_KINDS = [
  "folder",
  "url",
  "rss",
  "github_issues",
  "github_repo",
  "jira_issues",
  "s3",
  "azure_blob",
  "gcs",
  "webdav",
  "notion",
  "api",
  "statements",
  "custom",
] as const;

export type CreatableSourceKind = (typeof CREATABLE_SOURCE_KINDS)[number];

/** 库里还会有两种人建不出来的：每个库自带的 `memory`，与老数据的 `upload` */
export type SourceKind = CreatableSourceKind | "memory" | "upload";

/**
 * GitHub、Jira 来源同步缺不得的几项填了没有。缺了照样建得出来、存得下，之后每一轮同步
 * 都只会报错。仓库要是 `owner/name`：粘一整条 GitHub 链接进来，后端同样每轮拒收
 */
export function hasSyncFields(
  kind: string,
  f: { repo?: string; jiraUrl?: string; jiraProject?: string },
): boolean {
  if (kind === "github_issues" || kind === "github_repo") {
    return /^[^/\s]+\/[^/\s]+$/.test(f.repo?.trim() ?? "");
  }
  if (kind === "jira_issues") return Boolean(f.jiraUrl?.trim() && f.jiraProject?.trim());
  return true;
}
