type Grouped = { value: string; group?: string };

export type HeadedRow<T> =
  | { kind: "heading"; name: string }
  | { kind: "option"; option: T; index: number };

/** 选项列表里，组变了的地方插一行组名。`index` 是选项在原列表里的位置——
 *  键盘的上下键按它走，组名行不占位置，选不中 */
export function withGroupHeadings<T extends Grouped>(options: T[]): HeadedRow<T>[] {
  return options.flatMap((option, index) => {
    const opensGroup = !!option.group && option.group !== options[index - 1]?.group;
    const row: HeadedRow<T> = { kind: "option", option, index };
    return opensGroup ? [{ kind: "heading", name: option.group! }, row] : [row];
  });
}
