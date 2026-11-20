import { render, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import {
  EditorShownProvider,
  registerToolWindow,
  useContributions,
  useToolWindowShown,
  useWorkbenchChrome,
} from "../contributions";
import { ToolWindowEditor } from "./ToolWindowEditor";
import { ToolWindowView, ToolWindowHeader, useOwnToolWindowHeader } from "./ToolWindows";

afterEach(() => useWorkbenchChrome.setState({ primary: null }));

it("fills the editor tab with native content, custom headers and visibility without a second dock instance", () => {
  const stop = registerToolWindow({
    id: "editor-fixture",
    title: "Fixture",
    icon: "plugin",
    anchor: "bottom",
    component: function FixtureContent() {
      useOwnToolWindowHeader(true);
      const shown = useToolWindowShown();
      return (
        <>
          <ToolWindowHeader>
            <span>Native content tabs</span>
          </ToolWindowHeader>
          <input aria-label="Tool draft" defaultValue={shown ? "Visible" : "Hidden"} />
        </>
      );
    },
  });
  useWorkbenchChrome.setState({ primary: () => null, workspaceOpen: true });
  const tool = useContributions.getState().toolWindows.get("editor-fixture")!;
  const view = render(
    <>
      <ToolWindowView tw={tool} hidden />
      <EditorShownProvider value={true}>
        <ToolWindowEditor input={{ kind: "workspace.tool", key: "fixture", data: { tool: tool.id } }} />
      </EditorShownProvider>
    </>,
  );
  expect(screen.getAllByRole("textbox", { name: "Tool draft", hidden: true })).toHaveLength(1);
  expect(screen.getByRole("textbox", { name: "Tool draft" })).toHaveValue("Visible");
  expect(screen.getByText("Native content tabs")).toBeVisible();
  expect(screen.queryByRole("button", { name: "Hide" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Options" })).not.toBeInTheDocument();
  view.unmount();
  stop();
});
