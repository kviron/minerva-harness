import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button, Input, T } from "@/ui";
import styles from "./Workspace.module.css";

export function browserUrl(value: string): string | null {
  if (value.length > 8192) return null;
  try {
    const url = new URL(value);
    return ["https:", "http:"].includes(url.protocol) &&
      !url.username &&
      !url.password &&
      !["ipc.localhost", "openidea-plugin.localhost"].includes(url.hostname)
      ? url.href
      : null;
  } catch {
    return null;
  }
}

/** Remote pages never inherit the app origin or its native IPC capabilities. */
export function BrowserEditor() {
  const [address, setAddress] = useState("https://example.com");
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  return (
    <section className={styles.browser} aria-label="Browser">
      <form
        className={styles.address}
        onSubmit={(event) => {
          event.preventDefault();
          const next = browserUrl(address.trim());
          setError(null);
          setInvalid(!next);
          if (next) setUrl(next);
        }}
      >
        <Input
          aria-label="Web address"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
          invalid={invalid}
        />
        <Button type="submit">
          <T>Open</T>
        </Button>
        {url && (
          <Button variant="ghost" onClick={() => void openUrl(url).catch((error) => setError(String(error)))}>
            <T>Open in Browser</T>
          </Button>
        )}
      </form>
      {error && (
        <p role="alert" className={styles.note}>
          {error}
        </p>
      )}
      {invalid && (
        <p role="alert" className={styles.note}>
          <T>Enter an HTTP or HTTPS address.</T>
        </p>
      )}
      <p className={styles.note}>
        <T>Some websites block embedded previews. Open them in your browser.</T>
      </p>
      {url && <iframe title="Web preview" src={url} sandbox="allow-scripts allow-forms" referrerPolicy="no-referrer" />}
    </section>
  );
}
