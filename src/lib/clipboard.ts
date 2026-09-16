import { toast } from "sonner";

/** 复制文本到剪贴板；失败时给出可读提示。用于接入指引 / Base URL / 一次性 Key。 */
export async function copyText(text: string, label = "已复制"): Promise<void> {
  if (!text) return;
  try {
    await navigator.clipboard.writeText(text);
    toast.success(label);
  } catch {
    toast.error("复制失败，请手动选择后复制");
  }
}
