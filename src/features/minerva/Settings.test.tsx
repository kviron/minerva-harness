import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { MinervaSettings } from "./Settings";

const rpc = vi.hoisted(() => vi.fn());
vi.mock("@/platform/minerva", () => ({ minervaCall: rpc }));

beforeEach(() => {
  rpc.mockReset();
  rpc.mockImplementation(async (method: string) => {
    if (method === "providers.list" || method === "providers.save") return [];
    if (method === "codex.status") return { state: "signed-out" };
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
    if (method === "providers.discover") return ["one", "two", "three", "one"];
    throw new Error(method);
  });
});

it("sends API configuration through the backend and clears the key after saving", async () => {
  render(<MinervaSettings />);
  expect(screen.getByRole("region", { name: "Minerva" })).toBeInTheDocument();

  await waitFor(() => expect(screen.getByRole("button", { name: "Add model provider…" })).toBeEnabled());
  expect(rpc.mock.calls.filter(([method]) => method === "providers.list")).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "Add model provider…" }));
  await screen.findByLabelText("Provider ID");
  fireEvent.change(screen.getByLabelText("Provider ID"), { target: { value: "local" } });
  fireEvent.change(screen.getByLabelText("Provider name"), { target: { value: "Local" } });
  fireEvent.change(screen.getByLabelText("API base URL"), { target: { value: "https://example.test/v1" } });
  fireEvent.click(
    within(screen.getByRole("dialog", { name: "Add model provider…" })).getByRole("button", {
      name: "Fetch model list",
    }),
  );
  await screen.findByText("three");
  fireEvent.click(screen.getAllByRole("button", { name: "Move model up" })[1]!);
  fireEvent.click(screen.getAllByRole("button", { name: "Remove model" })[2]!);
  fireEvent.change(screen.getByLabelText("API key"), { target: { value: "fixture-key" } });
  fireEvent.click(
    within(screen.getByRole("dialog", { name: "Add model provider…" })).getByRole("button", { name: "Save" }),
  );
  await waitFor(() =>
    expect(rpc).toHaveBeenCalledWith("providers.save", {
      id: "local",
      name: "Local",
      baseUrl: "https://example.test/v1",
      models: ["two", "one"],
      apiKey: "fixture-key",
      protocol: "openai-chat",
      isNew: true,
    }),
  );
  await screen.findByRole("button", { name: "Add model provider…" });
  expect(screen.queryByLabelText("API key")).not.toBeInTheDocument();
});

it("fills the endpoint from a known service so only the key is typed", async () => {
  render(<MinervaSettings />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Add model provider…" })).toBeEnabled());
  fireEvent.click(screen.getByRole("button", { name: "Add model provider…" }));
  await screen.findByLabelText("Provider ID");
  fireEvent.click(screen.getByLabelText("Known service"));
  const options = await screen.findAllByText("DeepSeek");
  fireEvent.click(options[options.length - 1]!);
  await waitFor(() => expect(screen.getByLabelText("API base URL")).toHaveValue("https://api.deepseek.com"));
  expect(screen.getByLabelText("Provider ID")).toHaveValue("deepseek");
  expect(screen.getByText("deepseek-chat")).toBeInTheDocument();
});

it("lists all saved providers and opens only the selected provider", async () => {
  const provider = { baseUrl: "https://example.test/v1", protocol: "openai-chat", hasKey: true };
  const fallback = rpc.getMockImplementation()!;
  rpc.mockImplementation(async (method: string) =>
    method === "providers.list"
      ? [
          { ...provider, id: "first", name: "First provider", models: ["first-model"] },
          { ...provider, id: "second", name: "Second provider", models: ["second-model"] },
        ]
      : fallback(method),
  );
  render(<MinervaSettings />);
  expect(await screen.findByText(/First provider/)).toBeInTheDocument();
  expect(screen.getByText(/Second provider/)).toBeInTheDocument();
  expect(screen.queryByLabelText("Provider ID")).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole("button", { name: "Add model provider…" })).toBeEnabled());
  fireEvent.mouseDown(screen.getByText("Second provider"));
  await screen.findByLabelText("Provider ID");
  expect(screen.getByLabelText("Provider ID")).toHaveValue("second");
  expect(screen.getByText("second-model")).toBeInTheDocument();
  expect(screen.getByText(/First provider/)).toBeInTheDocument();

  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(screen.getByText(/First provider/)).toBeInTheDocument();
  expect(screen.getAllByText(/Second provider/)).toHaveLength(2);
});
