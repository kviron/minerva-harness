import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { CodexSettings } from "./CodexSettings";

const rpc = vi.hoisted(() => vi.fn());
const open = vi.hoisted(() => vi.fn());
vi.mock("@/platform/minerva", () => ({ minervaCall: rpc }));
vi.mock("@/platform/openUrl", () => ({ openExternalUrl: open }));

beforeEach(() => {
  rpc.mockReset();
  open.mockReset();
  open.mockResolvedValue(undefined);
  rpc.mockImplementation(async (method: string) => {
    if (method === "codex.status") return { state: "signed-out" };
    throw new Error(method);
  });
});

it("opens managed sign-in without displaying or collecting credentials", async () => {
  render(<CodexSettings models={[]} />);
  const button = await screen.findByRole("button", { name: "Sign in with ChatGPT" });
  await waitFor(() => expect(button).toBeEnabled());
  rpc.mockImplementation(async () => ({ state: "signing-in", authorizationUrl: "https://auth.openai.com/authorize" }));
  fireEvent.click(button);
  await screen.findByText("Waiting for browser sign-in…");
  expect(open).toHaveBeenCalledWith("https://auth.openai.com/authorize");
  expect(screen.queryByLabelText("API key")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Fetch model list" })).toBeDisabled();
});

it("selects catalog models and saves only their identifiers", async () => {
  rpc.mockImplementation(async (method: string) => {
    if (method === "codex.status") return { state: "signed-in" };
    if (method === "codex.models") return ["model-a", "model-b"];
    if (method === "codex.configure") return [];
    throw new Error(method);
  });
  render(<CodexSettings models={[]} />);
  await screen.findByText("Signed in");
  fireEvent.click(screen.getByRole("button", { name: "Fetch model list" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "model-b" }));
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(rpc).toHaveBeenCalledWith("codex.configure", { models: ["model-b"] }));
});

it("shows failed sign-in and releases the busy controls", async () => {
  render(<CodexSettings models={[]} />);
  const button = screen.getByRole("button", { name: "Sign in with ChatGPT" });
  await waitFor(() => expect(button).toBeEnabled());
  rpc.mockRejectedValueOnce(new Error("Login unavailable"));
  fireEvent.click(button);
  expect(await screen.findByRole("alert")).toHaveTextContent("Login unavailable");
  expect(button).toBeEnabled();
});

it("polls a pending browser sign-in and allows signing out afterwards", async () => {
  render(<CodexSettings models={[]} />);
  const button = screen.getByRole("button", { name: "Sign in with ChatGPT" });
  await waitFor(() => expect(button).toBeEnabled());
  rpc.mockImplementation(async (method: string) => {
    if (method === "codex.signIn") return { state: "signing-in" };
    if (method === "codex.status") return { state: "signed-in" };
    if (method === "codex.signOut") return { state: "signed-out" };
    throw new Error(method);
  });
  fireEvent.click(button);
  await screen.findByText("Signed in", {}, { timeout: 4000 });
  fireEvent.click(screen.getByRole("button", { name: "Sign out" }));
  await screen.findByText("Not signed in");
});

it("opens the browser when authorization URL arrives after sign-in started", async () => {
  render(<CodexSettings models={[]} />);
  const button = screen.getByRole("button", { name: "Sign in with ChatGPT" });
  await waitFor(() => expect(button).toBeEnabled());
  rpc.mockImplementation(async (method: string) => {
    if (method === "codex.signIn") return { state: "signing-in" };
    if (method === "codex.status")
      return { state: "signing-in", authorizationUrl: "https://auth.openai.com/authorize" };
    throw new Error(method);
  });
  fireEvent.click(button);
  await waitFor(() => expect(open).toHaveBeenCalledWith("https://auth.openai.com/authorize"), { timeout: 4000 });
  expect(open).toHaveBeenCalledTimes(1);
});

it("disables account controls while another window signs out and polls completion", async () => {
  rpc.mockImplementation(async (method: string) => {
    if (method === "codex.status") return { state: "signing-out" };
    throw new Error(method);
  });
  render(<CodexSettings models={[]} />);
  await screen.findByText("Signing out…");
  expect(screen.getByRole("button", { name: "Sign in with ChatGPT" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
  rpc.mockImplementation(async () => ({ state: "signed-out" }));
  await screen.findByText("Not signed in", {}, { timeout: 4000 });
  expect(screen.getByRole("button", { name: "Sign in with ChatGPT" })).toBeEnabled();
});
