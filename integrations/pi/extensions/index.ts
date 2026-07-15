import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

import { registerPhuxExtension } from "../src/extension.js";

export default function phuxExtension(pi: ExtensionAPI): void {
  registerPhuxExtension(pi);
}
