import { createContext, useContext, type ReactNode } from "react";
import { EyeOff } from "lucide-react";

/** Current settings search text; rows that do not match it are hidden. */
export const SettingsQuery = createContext("");

/** Hide settings, as in Spicy Lyrics: while hiding is allowed, chosen rows
 *  leave the settings window, and each row can offer a button to hide it. */
export const HiddenSettings = createContext<{
  allow: boolean;
  buttons: boolean;
  hidden: ReadonlySet<string>;
  hide: (id: string) => void;
}>({ allow: false, buttons: false, hidden: new Set(), hide: () => {} });

export function matchesSetting(
  query: string,
  ...texts: (string | undefined)[]
) {
  const needle = query.trim().toLowerCase();
  return !needle || texts.some((text) => text?.toLowerCase().includes(needle));
}

export function SettingRow({
  label,
  help,
  keywords,
  stacked = false,
  hideId,
  unavailable,
  children,
}: {
  label: ReactNode;
  help?: ReactNode;
  keywords?: string;
  stacked?: boolean;
  /** Lets the row be hidden; set by the settings window. */
  hideId?: string;
  /** Greys the row out and says why it does nothing right now. */
  unavailable?: string;
  children?: ReactNode;
}) {
  const query = useContext(SettingsQuery);
  const hiding = useContext(HiddenSettings);
  const text = [label, help].filter(
    (part): part is string => typeof part === "string",
  );
  if (!matchesSetting(query, ...text, keywords)) return null;
  if (hideId && hiding.allow && hiding.hidden.has(hideId)) return null;
  return (
    <div
      className={`setting-row ${stacked ? "stacked" : ""} ${unavailable ? "unavailable" : ""}`}
    >
      <div className="setting-text">
        <span className="setting-label">
          {label}
          {hideId && hiding.allow && hiding.buttons && (
            <button
              className="setting-hide"
              title={`Hide ${typeof label === "string" ? label : "this setting"}`}
              onClick={() => hiding.hide(hideId)}
            >
              <EyeOff size={13} />
            </button>
          )}
        </span>
        {help && <p className="setting-help">{help}</p>}
        {unavailable && <p className="setting-unavailable">{unavailable}</p>}
      </div>
      {children !== undefined && (
        <div className="setting-control" inert={!!unavailable}>
          {children}
        </div>
      )}
    </div>
  );
}

export function Toggle({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <input
      className="toggle"
      type="checkbox"
      role="switch"
      aria-label={label}
      checked={checked}
      onChange={(event) => onChange(event.target.checked)}
    />
  );
}
