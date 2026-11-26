import type { SourceView } from "../api";

/** 逗号分隔的 glob 拆成列表。留空给 undefined：JSON 里不出现这个键，后端按默认规则筛 */
export const globList = (value: string) => {
  // glob 的 [,] 能匹配文件名里的逗号；拆列表时不能把字符组也拆开。
  // ] 在组的第一位（或 ! 后第一位）是字面字符，后一个 ] 才闭合。
  const parts: string[] = [];
  let start = 0;
  let characterClass = -1;
  for (let i = 0; i < value.length; i++) {
    if (value[i] === "[" && characterClass < 0) characterClass = i;
    else if (value[i] === "]" && characterClass >= 0 &&
      i > characterClass + (value[characterClass + 1] === "!" ? 2 : 1)) {
      characterClass = -1;
    } else if (value[i] === "," && characterClass < 0) {
      parts.push(value.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(value.slice(start));
  const list = parts.map((g) => g.trim()).filter(Boolean);
  return list.length ? list : undefined;
};

/** 编辑 github_repo / gitlab_repo 来源时提交的配置。保存是整份替换，所以表单里不露面的键
 *  （比如 `max_file_bytes`）从库里那份原样带回去；令牌留空就不发，后端保留原值。
 *  GitLab 把项目路径存在 `project`，GitHub 存在 `repo` */
export function githubRepoConfig(
  saved: NonNullable<SourceView["config"]>,
  form: { repo: string; authHeader: string; include: string; exclude: string },
  key: "repo" | "project" = "repo",
) {
  return {
    ...saved,
    [key]: form.repo.trim(),
    ...(form.authHeader.trim() ? { auth_header: form.authHeader.trim() } : {}),
    include: globList(form.include),
    exclude: globList(form.exclude),
  };
}
