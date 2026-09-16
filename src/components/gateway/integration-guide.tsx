import { useState } from "react";
import { Check, Copy } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { copyText } from "@/lib/clipboard";
import { REGIONS, regionDescriptor } from "@/lib/region";
import { cn } from "@/lib/utils";
import type { ApiKeyRecord, Region } from "@/lib/types";
import { useGatewayStore } from "@/stores/gateway";

const TOOLS = [
  { key: "cursor", label: "Cursor" },
  { key: "cline", label: "Cline" },
  { key: "continue", label: "Continue" },
  { key: "claude-code", label: "Claude Code" },
  { key: "openwebui", label: "OpenWebUI" },
  { key: "cherry", label: "Cherry Studio" },
] as const;

type ToolKey = (typeof TOOLS)[number]["key"];

/** 取该 region 第一个启用中的 Key 前缀；无则给出占位提示。 */
function representativeKey(keys: ApiKeyRecord[], region: Region): string {
  const active = keys.find((key) => key.region === region && !key.revoked);
  return active ? `${active.prefix}…` : "sk-wb-…（请先在上方创建 Key）";
}

function snippetFor(tool: ToolKey, baseUrl: string, rootUrl: string, key: string): string {
  switch (tool) {
    case "cursor":
      return [
        "Cursor → Settings → Models → OpenAI API Key",
        "",
        `Override OpenAI Base URL: ${baseUrl}`,
        `API Key:  ${key}`,
        "Model:    在模型列表中选择，如 GLM-5.3",
      ].join("\n");
    case "cline":
      return JSON.stringify(
        {
          apiProvider: "openai",
          openAiBaseUrl: baseUrl,
          openAiApiKey: key,
          openAiModelId: "GLM-5.3",
        },
        null,
        2,
      );
    case "continue":
      return [
        "models:",
        "  - name: WorkBuddy",
        "    provider: openai",
        "    model: GLM-5.3",
        `    apiBase: ${baseUrl}`,
        `    apiKey: ${key}`,
      ].join("\n");
    case "claude-code":
      return [`export ANTHROPIC_BASE_URL=${rootUrl}`, `export ANTHROPIC_AUTH_TOKEN=${key}`].join("\n");
    case "openwebui":
      return [`Base URL: ${baseUrl}`, `API Key:  ${key}`].join("\n");
    case "cherry":
      return [`API 地址: ${baseUrl}`, `API 密钥: ${key}`, "模型: 在模型列表中选择"].join("\n");
    default:
      return "";
  }
}

/** 接入指引 Tabs（Cursor / Cline / Continue / Claude Code / OpenWebUI / Cherry Studio），带复制按钮（P0-9）。 */
export function IntegrationGuide({ baseUrl, className }: { baseUrl: string; className?: string }) {
  const keys = useGatewayStore((s) => s.keys);
  const [tool, setTool] = useState<ToolKey>("cursor");
  const [region, setRegion] = useState<Region>("cn");
  const [copied, setCopied] = useState(false);

  const rootUrl = baseUrl.replace(/\/v1\/?$/, "");
  const key = representativeKey(keys, region);
  const snippet = snippetFor(tool, baseUrl, rootUrl, key);

  async function onCopy() {
    await copyText(snippet, "代码已复制");
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  }

  return (
    <Card className={cn("gap-0 py-0", className)}>
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/60 px-5 py-3">
        <span className="text-sm font-semibold">接入指引</span>
        <div className="flex flex-wrap items-center gap-2">
          <Tabs value={region} onValueChange={(value) => setRegion(value as Region)}>
            <TabsList>
              {REGIONS.map((r) => (
                <TabsTrigger key={r} value={r}>
                  {regionDescriptor(r).versionLabel} Key
                </TabsTrigger>
              ))}
            </TabsList>
          </Tabs>
          <Button variant="outline" size="sm" onClick={() => void onCopy()}>
            {copied ? <Check /> : <Copy />}
            复制代码
          </Button>
        </div>
      </div>

      <Tabs value={tool} onValueChange={(value) => setTool(value as ToolKey)}>
        <div className="border-b border-border/60 px-5 py-2">
          <TabsList className="flex-wrap">
            {TOOLS.map((item) => (
              <TabsTrigger key={item.key} value={item.key}>
                {item.label}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        <div className="px-5 py-4">
          <pre className="min-w-0 overflow-x-auto rounded-lg border border-border bg-muted/40 p-4 font-mono text-xs leading-6">
            {snippet}
          </pre>
          <p className="mt-2 text-xs text-muted-foreground">
            建议开启流式（stream）；非流式请求会由网关聚合后一次性返回，首字节延迟较长。
          </p>
        </div>
      </Tabs>
    </Card>
  );
}
