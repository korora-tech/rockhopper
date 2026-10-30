// Sample data for the launcher demo on the home page. It mirrors what
// Rockhopper lists, but nothing here runs: choosing a result only says what
// the real launcher would have done.

export type Kind = "app" | "window" | "command" | "clipboard";

export interface Item {
  kind: Kind;
  name: string;
  detail: string;
  keywords?: string[];
  /** Colour of the initial drawn for apps and windows. */
  colour?: string;
  /** What the real launcher would do on enter. */
  does: string;
  /** Opens another mode instead of doing something. */
  opens?: Mode;
  /** Needs a second enter, like Log Out. */
  confirm?: boolean;
}

export interface Emoji {
  char: string;
  name: string;
  code: string;
}

export type Mode = "root" | "commands" | "emoji" | "clipboard";

export const MODE_LABEL: Record<Mode, string> = {
  root: "",
  commands: "commands",
  emoji: "emoji",
  clipboard: "clipboard",
};

export const KIND_LABEL: Record<Kind, string> = {
  app: "Application",
  window: "Window",
  command: "Command",
  clipboard: "",
};

export const PREFIXES: [string, Mode][] = [
  [">", "commands"],
  ["e ", "emoji"],
];

const app = (name: string, detail: string, colour: string, keywords: string[] = []): Item => ({
  kind: "app",
  name,
  detail,
  colour,
  keywords,
  does: `Opens ${name}`,
});

const win = (name: string, detail: string, colour: string): Item => ({
  kind: "window",
  name,
  detail,
  colour,
  does: `Switches to ${detail.split(" · ")[0]}`,
});

const command = (name: string, detail: string, extra: Partial<Item> = {}): Item => ({
  kind: "command",
  name,
  detail,
  does: `Runs ${detail}`,
  ...extra,
});

export const APPS: Item[] = [
  app("Firefox", "Web Browser", "#e66000", ["browser", "internet", "web"]),
  app("Ghostty", "Terminal Emulator", "#3551f3", ["terminal", "shell"]),
  app("Zed", "Code Editor", "#0751cf", ["editor", "code"]),
  app("Files", "Access and organise files", "#4a86c8", ["nautilus", "folder", "explorer"]),
  app("Signal", "Private Messenger", "#3a76f0", ["chat"]),
  app("Spotify", "Music for everyone", "#1db954", ["music"]),
  app("Thunderbird", "Mail Client", "#0a84ff", ["email", "mail"]),
  app("Obsidian", "Knowledge base", "#7c3aed", ["notes", "markdown"]),
  app("GIMP", "GNU Image Manipulation Program", "#8a7a5e", ["image", "photo", "paint"]),
  app("Blender", "3D modelling and rendering", "#e87d0d", ["3d"]),
  app("Steam", "Game library", "#1b2838", ["games"]),
  app("Settings", "System Settings", "#6b7280", ["preferences", "control"]),
  app("Calculator", "Perform calculations", "#5b6170", ["math"]),
  app("Krita", "Digital painting", "#3daee9", ["draw", "paint"]),
  app("Element", "Matrix client", "#0dbd8b", ["chat", "matrix"]),
];

export const WINDOWS: Item[] = [
  win("rockhopper — src/search.rs", "Zed · workspace 1", "#0751cf"),
  win("korora-tech/rockhopper", "Firefox · workspace 2", "#e66000"),
  win("~/rockhopper — cargo test", "Ghostty · workspace 1", "#3551f3"),
  win("Weekly notes", "Obsidian · workspace 3", "#7c3aed"),
];

export const COMMANDS: Item[] = [
  command("Lock Screen", "loginctl lock-session", { keywords: ["lock"] }),
  command("Suspend", "systemctl suspend", { keywords: ["sleep"] }),
  command("Log Out", "niri msg action quit", { confirm: true, keywords: ["logout", "exit"] }),
  command("Restart", "systemctl reboot", { confirm: true, keywords: ["reboot"] }),
  command("Shut Down", "systemctl poweroff", { confirm: true, keywords: ["poweroff", "off"] }),
  command("Clipboard History", "rockhopper clipboard", {
    opens: "clipboard",
    keywords: ["paste", "copy"],
  }),
  command("Search Emoji", "rockhopper emoji", { opens: "emoji", keywords: ["emoji"] }),
  command("Toggle Fullscreen", "niri msg action fullscreen-window"),
  command("Float Window", "niri msg action toggle-window-floating"),
  command("Center Window", "niri msg action center-window"),
  command("Maximize Column", "niri msg action maximize-column"),
  command("Move Window to Next Monitor", "niri msg action move-window-to-monitor-next"),
  command("Open Overview", "niri msg action open-overview"),
];

export const CLIPBOARD: Item[] = [
  {
    kind: "clipboard",
    name: "nix run github:korora-tech/rockhopper",
    detail: "Text · just now",
    does: "Puts it back on the clipboard",
  },
  {
    kind: "clipboard",
    name: "spawn-at-startup \"rockhopper\" \"daemon\"",
    detail: "Text · 4 minutes ago",
    does: "Puts it back on the clipboard",
  },
  {
    kind: "clipboard",
    name: "https://github.com/helix-editor/nucleo",
    detail: "Text · 22 minutes ago",
    does: "Puts it back on the clipboard",
  },
  {
    kind: "clipboard",
    name: "screenshot-2026-10-29.png",
    detail: "Image · 1280 × 720 · 1 hour ago",
    does: "Puts it back on the clipboard",
  },
  {
    kind: "clipboard",
    name: "Mod+Space { spawn \"rockhopper\"; }",
    detail: "Text · yesterday",
    does: "Puts it back on the clipboard",
  },
];

export const EMOJI: Emoji[] = [
  { char: "🎉", name: "party popper", code: "tada" },
  { char: "🐧", name: "penguin", code: "penguin" },
  { char: "🚀", name: "rocket", code: "rocket" },
  { char: "👍", name: "thumbs up", code: "+1" },
  { char: "❤️", name: "red heart", code: "heart" },
  { char: "😂", name: "face with tears of joy", code: "joy" },
  { char: "🔥", name: "fire", code: "fire" },
  { char: "✨", name: "sparkles", code: "sparkles" },
  { char: "🥳", name: "partying face", code: "partying_face" },
  { char: "🎊", name: "confetti ball", code: "confetti_ball" },
  { char: "👀", name: "eyes", code: "eyes" },
  { char: "🙏", name: "folded hands", code: "pray" },
  { char: "✅", name: "check mark button", code: "white_check_mark" },
  { char: "🐛", name: "bug", code: "bug" },
  { char: "🦀", name: "crab", code: "crab" },
  { char: "☕", name: "hot beverage", code: "coffee" },
  { char: "🤔", name: "thinking face", code: "thinking" },
  { char: "😅", name: "grinning face with sweat", code: "sweat_smile" },
  { char: "🙌", name: "raising hands", code: "raised_hands" },
  { char: "💡", name: "light bulb", code: "bulb" },
  { char: "📦", name: "package", code: "package" },
  { char: "🔒", name: "locked", code: "lock" },
  { char: "⚡", name: "high voltage", code: "zap" },
  { char: "🌊", name: "water wave", code: "ocean" },
  { char: "🧊", name: "ice", code: "ice_cube" },
  { char: "❄️", name: "snowflake", code: "snowflake" },
  { char: "🐟", name: "fish", code: "fish" },
  { char: "🎯", name: "direct hit", code: "dart" },
  { char: "🧪", name: "test tube", code: "test_tube" },
  { char: "🛠️", name: "hammer and wrench", code: "hammer_and_wrench" },
  { char: "📋", name: "clipboard", code: "clipboard" },
  { char: "⌨️", name: "keyboard", code: "keyboard" },
];

/** Shown for an empty root search: what you open most. */
export const FREQUENT = [APPS[0], APPS[1], WINDOWS[0], APPS[2], COMMANDS[0], APPS[4], APPS[3]];
