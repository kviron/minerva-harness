import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { SessionProviderPicker } from "./SessionProviderPicker";

it("shows an unavailable current provider and sends only an explicit selection", () => {
  const change = vi.fn();
  render(
    <SessionProviderPicker
      value="minerva.api.removed"
      name="Removed provider"
      options={[{ value: "minerva.api.b", label: "Provider B" }]}
      disabled={false}
      onChange={change}
    />,
  );
  expect(screen.getByRole("button", { name: "Provider" })).toHaveTextContent("Removed provider");
  expect(change).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Provider" }));
  fireEvent.click(screen.getByRole("option", { name: "Provider B" }));
  expect(change).toHaveBeenCalledExactlyOnceWith("minerva.api.b");
});

it("disables selection while a task is working", () => {
  render(
    <SessionProviderPicker
      value="a"
      name="Provider A"
      options={[{ value: "b", label: "Provider B" }]}
      disabled
      onChange={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "Provider" })).toBeDisabled();
});
