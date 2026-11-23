import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { MinervaSettings } from "./Settings";

const rpc = vi.hoisted(() => vi.fn());
vi.mock("@/platform/minerva", () => ({ minervaCall: rpc }));

beforeEach(() => {
  rpc.mockReset();
  rpc.mockImplementation(async (method: string) => {
    if (method === "providers.list" || method === "providers.save") return [];
    if (method === "providers.known")
      return [
        {
          id: "deepseek",
          name: "DeepSeek",
          baseUrl: "https://api.deepseek.com",
          protocol: "openai-chat",
          models: ["deepseek-chat"],
          envVar: "DEEPSEEK_API_KEY",
        },
      ];
    if (method === "plugins.list")
      return [{ id: "test", name: "Test package", description: "", enabled: false, canToggle: true, error: null }];
    if (method === "plugins.setEnabled") throw new Error("package could not start");
    throw new Error(method);
  });
});

it("sends API configuration through the backend and clears the key after saving", async () => {
  render(<MinervaSettings />);
  await screen.findByText("Test package");
  fireEvent.change(screen.getByLabelText("Provider ID"), { target: { value: "local" } });
  fireEvent.change(screen.getByLabelText("Provider name"), { target: { value: "Local" } });
  fireEvent.change(screen.getByLabelText("API base URL"), { target: { value: "https://example.test/v1" } });
  fireEvent.change(screen.getByLabelText("Models separated by commas"), { target: { value: "one, two ," } });
  fireEvent.change(screen.getByLabelText("API key"), { target: { value: "fixture-key" } });
  fireEvent.click(screen.getByRole("button", { name: "Save API provider" }));
  await waitFor(() =>
    expect(rpc).toHaveBeenCalledWith("providers.save", {
      id: "local",
      name: "Local",
      baseUrl: "https://example.test/v1",
      models: ["one", "two"],
      apiKey: "fixture-key",
      protocol: "openai-chat",
      isNew: true,
    }),
  );
  await waitFor(() => expect(screen.getByLabelText("API key")).toHaveValue(""));
});

it("fills the endpoint from a known service so only the key is typed", async () => {
  render(<MinervaSettings />);
  await screen.findByText("Test package");
  fireEvent.click(screen.getByLabelText("Known service"));
  const options = await screen.findAllByText("DeepSeek");
  fireEvent.click(options[options.length - 1]!);
  await waitFor(() => expect(screen.getByLabelText("API base URL")).toHaveValue("https://api.deepseek.com"));
  expect(screen.getByLabelText("Provider ID")).toHaveValue("deepseek");
  expect(screen.getByLabelText("Models separated by commas")).toHaveValue("deepseek-chat");
});

it("keeps failed plugin enablement visible and does not pretend the package started", async () => {
  render(<MinervaSettings />);
  const toggle = await screen.findByRole("checkbox", { name: "Test package" });
  fireEvent.click(toggle);
  await screen.findByRole("alert");
  expect(screen.getByRole("alert")).toHaveTextContent("package could not start");
  expect(toggle).not.toBeChecked();
});
