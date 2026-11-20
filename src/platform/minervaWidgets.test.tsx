import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { Button, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./minervaWidgets";

it("renders an existing SDK compound select through the IDE widget", async () => {
  const change = vi.fn();
  render(
    <Select value="one" onValueChange={change}>
      <SelectTrigger aria-label="Fixture model">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="one">First model</SelectItem>
        <SelectItem value="two">Second model</SelectItem>
      </SelectContent>
    </Select>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Fixture model" }));
  fireEvent.click(await screen.findByRole("option", { name: "Second model" }));
  expect(change).toHaveBeenCalledWith("two");
});

it("uses the host button and preserves plugin click handlers", () => {
  const click = vi.fn();
  render(
    <Button variant="outline" size="sm" onClick={click}>
      Plugin action
    </Button>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Plugin action" }));
  expect(click).toHaveBeenCalledOnce();
});
