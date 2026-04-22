---
id: image_generation
name: Image Generation
description: Generate images using text-to-image models via Happy Server proxy
category: system
version: "2.0.0"
supports_background: true
should_defer: true
search_hint: "generate image text-to-image AI drawing"
is_read_only: false
is_concurrency_safe: false
---

# Image Generation Tool

Generate images from text descriptions. Images are generated via Happy Server proxy and billed per image from user balance.

## Available Models & Pricing

| Model | Price | Description |
|-------|-------|-------------|
| `qwen-image-max` (default) | ¥0.20 | 高质量图像 (固定尺寸) |
| `qwen-image-2.0-pro` | ¥0.20 | 最强文字渲染 + 自定义分辨率 |
| `qwen-image-plus` | ¥0.08 | 快速图像生成 |
| `wan2.6-t2i` | ¥0.08 | 写实摄影 + 文字渲染 |

## Parameters

```json
{
  "prompt": "string (required) - Image description in Chinese or English",
  "model": "string (optional) - Model name, default: qwen-image-max",
  "size": "string (optional) - Image size, default: 1280*1280. Supported: 1280*1280, 1024*1024, 720*1280, 1280*720",
  "negative_prompt": "string (optional) - What to avoid in the image",
  "seed": "integer (optional) - Random seed for reproducibility (0-2147483647)",
  "notify": "boolean (optional) - Send notification when done, default: true",
  "timeout": "integer (optional) - Timeout in seconds, default: 300"
}
```

## Examples

**Basic usage** (returns immediately with run_id):
```json
{
  "prompt": "一只可爱的橙色小猫坐在窗台上，阳光洒在它的毛发上，水彩画风格"
}
```

**With custom parameters**:
```json
{
  "prompt": "未来城市的全景图，赛博朋克风格，霓虹灯闪烁，超高清细节",
  "model": "qwen-image-2.0-pro",
  "size": "1280*720",
  "negative_prompt": "模糊，低质量，变形，扭曲",
  "seed": 42
}
```

## Output

Returns run_id immediately, then sends notification with local file path when complete.

## Notes

- **Background Only**: All executions run in background, typically takes 10-60 seconds
- **Auto Download**: Images are automatically downloaded to the current working directory
- **Billing**: Charged per image from user balance
- **Prompt Tips**:
  - Be specific about style, composition, lighting
  - Use Chinese for better results
  - Include quality keywords like "高清", "细节丰富", "专业摄影"

## Agent Usage

When user requests image generation:
1. Call this tool with the enhanced prompt
2. The tool generates the image and **automatically downloads it to the current working directory**
3. You will receive a notification with the local file path when complete
4. Inform the user about the saved file location
