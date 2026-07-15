import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { registerPhuxExtension } from "../src/extension.js";

export default function phuxExtension(pi: ExtensionAPI): void {
  registerPhuxExtension(pi);
}
