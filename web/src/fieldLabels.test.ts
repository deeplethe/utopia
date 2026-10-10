import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Field, Input, Textarea } from "./ui";

describe("field labels", () => {
  it.each([createElement(Input), createElement(Textarea)])("associates a label with its text control", (control) => {
    const html = renderToStaticMarkup(createElement(Field, {
      label: "Name", children: control,
    }));
    const labelId = /<label[^>]*for="([^"]+)"/.exec(html)?.[1];
    expect(labelId).toBeTruthy();
    expect(html).toContain(`id="${labelId}"`);
  });
  it("retains an existing control id", () => {
    const html = renderToStaticMarkup(createElement(Field, {
      label: "Name", children: createElement(Input, { id: "existing-name" }),
    }));
    expect(html).toContain('for="existing-name"');
    expect(html).toContain('id="existing-name"');
  });
});
