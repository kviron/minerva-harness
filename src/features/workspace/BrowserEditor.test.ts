import { expect, it } from "vitest";
import { browserUrl } from "./BrowserEditor";

it("allows HTTP previews and rejects executable, privileged and credential-bearing addresses", () => {
  expect(browserUrl("https://example.com/a?q=1")).toBe("https://example.com/a?q=1");
  expect(browserUrl("http://localhost:3000")).toBe("http://localhost:3000/");
  for (const value of [
    "javascript:alert(1)",
    "file:///C:/secrets",
    "data:text/html,hello",
    "https://a:b@example.com",
    "http://ipc.localhost",
    "http://openidea-plugin.localhost",
    "bad",
    "x".repeat(8193),
  ])
    expect(browserUrl(value)).toBeNull();
});
