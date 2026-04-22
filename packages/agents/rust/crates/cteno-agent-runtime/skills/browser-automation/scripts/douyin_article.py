"""Douyin Article Publisher - 在抖音创作者中心发布文章。

用法:
    python3 douyin_article.py --title "文章标题" --summary "摘要" --body "正文内容" [--header-image /path/to/img.jpg] [--cover /path/to/cover.jpg]

参数:
    --title          文章标题（最多30字）
    --summary        文章摘要（最多30字，可选）
    --body           文章正文（纯文本，最少100字，换行用 \\n）
    --header-image   文章头图路径（显示在正文顶部，可选）
    --cover          封面图片路径（推荐信息流中的封面，可选）
    --dry-run        只填写不发布（调试用）
    --port           CDP 端口（默认 9222）

关键 URL:
    文章编辑器: https://creator.douyin.com/creator-micro/content/post/article?enter_from=publish_page&media_type=article&type=new

页面结构（2026-03 实测）:
    - 标题: input.semi-input[placeholder*="文章标题"]
    - 摘要: input.semi-input[placeholder*="摘要"]
    - 正文: div.tiptap.ProseMirror[contenteditable="true"]（TipTap 富文本编辑器）
    - 文章头图: "点击上传图片" 区域（显示在文章正文顶部）
    - 封面: "点击上传封面图" 区域（推荐信息流中的封面缩略图）
    - 发布: button 文本包含 "发布"（排除 "高清发布"）
"""
import argparse
import asyncio
import json
import sys
import os
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__)))
from cdp import CDPBrowser

def log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


async def fill_title(browser, sid, title):
    """填写文章标题（React 受控 input）。"""
    selector = 'input[placeholder*="文章标题"]'
    await browser.wait_for(selector, sid=sid)
    await browser.type_text(selector, title, sid=sid)
    # 验证
    value = await browser.evaluate(f"""
        document.querySelector('{selector}')?.value || ''
    """, sid=sid)
    if value != title:
        raise RuntimeError(f"Title verification failed: expected '{title}', got '{value}'")
    log(f"Title filled: {title}")


async def fill_summary(browser, sid, summary):
    """填写文章摘要。"""
    selector = 'input[placeholder*="摘要"]'
    await browser.wait_for(selector, sid=sid)
    await browser.type_text(selector, summary, sid=sid)
    log(f"Summary filled: {summary}")


async def fill_body(browser, sid, body_text):
    """填写文章正文（TipTap ProseMirror 编辑器）。

    TipTap/ProseMirror 不是普通 input，不能用 value setter。
    使用 Input.insertText CDP 命令——最可靠的富文本编辑器输入方式。
    """
    editor_selector = 'div.tiptap.ProseMirror'
    await browser.wait_for(editor_selector, sid=sid)
    await browser.type_into_contenteditable(editor_selector, body_text, sid=sid)
    log(f"Body filled ({len(body_text)} chars)")


async def _upload_image(browser, sid, file_path, label, trigger_selector_js, confirm_button="完成"):
    """通用图片上传。

    Args:
        file_path: 图片文件绝对路径
        label: 日志标签
        trigger_selector_js: JS 表达式，定位触发器元素并设置 id，返回临时 id 或 null
        confirm_button: 上传后编辑弹窗的确认按钮文本（None 表示无弹窗）
    """
    abs_path = os.path.abspath(file_path)
    if not os.path.exists(abs_path):
        raise FileNotFoundError(f"{label} image not found: {abs_path}")

    # 关闭引导弹窗（可能遮挡上传区域）
    await browser.dismiss_dialogs(sid=sid)
    await asyncio.sleep(0.5)

    # 滚动到上传区域
    await browser.evaluate("window.scrollTo(0, 600)", sid=sid)
    await asyncio.sleep(1)

    # 用 JS 定位触发器并设置临时 id
    trigger_id = await browser.evaluate(trigger_selector_js, sid=sid)
    if not trigger_id:
        log(f"{label} upload trigger not found, skipping")
        return

    try:
        await browser.upload_file(f'#{trigger_id}', abs_path, sid=sid)
        await asyncio.sleep(3)

        # 上传后可能有编辑弹窗（裁剪/预览），点击确认按钮关闭
        if confirm_button:
            await asyncio.sleep(1)
            try:
                await browser.click_by_text(confirm_button, tag="button", sid=sid)
                log(f"{label} editor dialog closed")
            except Exception:
                await browser.dismiss_dialogs([confirm_button, "确定", "完成"], sid=sid)
            await asyncio.sleep(2)

        # 滚动回顶部
        await browser.evaluate("window.scrollTo(0, 0)", sid=sid)
        log(f"{label} uploaded: {abs_path}")
    except Exception as e:
        log(f"{label} upload failed: {e}")
        await browser.dismiss_dialogs(["完成", "确定", "取消", "关闭"], sid=sid)


async def upload_header_image(browser, sid, image_path):
    """上传文章头图（显示在文章正文顶部）。

    触发器是 +号图标（div.addIcon），不是文字 span。
    上传后弹出"图片编辑"对话框，需点击"确定"。
    """
    await _upload_image(
        browser, sid,
        file_path=image_path,
        label="Header image",
        # 头图触发器：找第一个 addIcon（+号），它在"点击上传图片"旁边
        trigger_selector_js="""
            (() => {
                const area = document.querySelector('[class*="content-upload-go"]');
                if (!area) return null;
                const icon = area.querySelector('[class*="addIcon"]');
                if (!icon) return null;
                icon.id = '__cdp_header_trigger';
                return '__cdp_header_trigger';
            })()
        """,
        confirm_button="确定",  # 头图编辑弹窗用"确定"
    )


async def upload_cover(browser, sid, cover_path):
    """上传封面图片（推荐信息流中的封面缩略图）。

    触发器是 span 文字"点击上传封面图"。
    上传后弹出"编辑封面"对话框，需点击"完成"。
    """
    await _upload_image(
        browser, sid,
        file_path=cover_path,
        label="Cover",
        # 封面触发器：文字 span "点击上传封面图"
        trigger_selector_js="""
            (() => {
                const spans = document.querySelectorAll('span');
                for (const s of spans) {
                    if (s.textContent.trim() === '点击上传封面图' && s.children.length === 0) {
                        s.id = '__cdp_cover_trigger';
                        return '__cdp_cover_trigger';
                    }
                }
                return null;
            })()
        """,
        confirm_button="完成",  # 封面编辑弹窗用"完成"
    )


async def click_publish(browser, sid):
    """点击发布按钮。"""
    clicked = await browser.evaluate("""
        (() => {
            const buttons = Array.from(document.querySelectorAll('button'));
            const pub = buttons.find(b => {
                const text = b.textContent.trim();
                return text === '发布' && !b.disabled;
            });
            if (pub) { pub.click(); return true; }
            return false;
        })()
    """, sid=sid)
    if not clicked:
        raise RuntimeError("Publish button not found or disabled")
    log("Publish button clicked")


async def verify_publish(browser, sid):
    """等待发布结果（URL 变化或成功提示）。"""
    for _ in range(20):
        url = await browser.evaluate("location.href", sid=sid)
        if "upload" in url or "manage" in url:
            log(f"Published! Redirected to: {url}")
            return True

        # 检查是否有错误提示
        error = await browser.evaluate("""
            (() => {
                const toast = document.querySelector('.semi-toast-content, [class*="toast"], [class*="error"]');
                return toast ? toast.textContent.trim().substring(0, 100) : null;
            })()
        """, sid=sid)
        if error:
            log(f"Warning: {error}")

        await asyncio.sleep(1)

    log("Publish result uncertain - check manually")
    return False


async def main():
    parser = argparse.ArgumentParser(description="Douyin Article Publisher")
    parser.add_argument("--title", required=True, help="文章标题")
    parser.add_argument("--summary", default="", help="文章摘要")
    parser.add_argument("--body", required=True, help="文章正文")
    parser.add_argument("--header-image", default="", help="文章头图路径（显示在正文顶部）")
    parser.add_argument("--cover", default="", help="封面图片路径（信息流缩略图）")
    parser.add_argument("--dry-run", action="store_true", help="只填写不发布")
    parser.add_argument("--port", type=int, default=9222, help="CDP 端口")
    args = parser.parse_args()

    article_url = (
        "https://creator.douyin.com/creator-micro/content/post/article"
        "?enter_from=publish_page&media_type=article&type=new"
    )

    async with CDPBrowser(port=args.port) as browser:
        # 1. 直接打开文章编辑器（不需要手动找 tab）
        log("Opening article editor...")
        sid = await browser.new_page(article_url, wait=5)

        url = await browser.evaluate("location.href", sid=sid)
        if "login" in url or "passport" in url:
            log("ERROR: Not logged in. Please login in Chrome first.")
            await browser.screenshot("/tmp/douyin_not_logged_in.png", sid=sid)
            print(json.dumps({"status": "error", "reason": "not_logged_in"}))
            return

        await browser.screenshot("/tmp/douyin_article_step0.png", sid=sid)

        # 2. 填写标题
        await fill_title(browser, sid, args.title)
        await browser.screenshot("/tmp/douyin_article_step1.png", sid=sid)

        # 3. 填写摘要（可选）
        if args.summary:
            await fill_summary(browser, sid, args.summary)

        # 4. 填写正文
        await fill_body(browser, sid, args.body)
        await browser.screenshot("/tmp/douyin_article_step2.png", sid=sid)

        # 5. 上传文章头图（可选，显示在正文顶部）
        if args.header_image:
            await upload_header_image(browser, sid, args.header_image)
            await browser.screenshot("/tmp/douyin_article_step3_header.png", sid=sid)

        # 6. 上传封面（可选，信息流缩略图）
        if args.cover:
            await upload_cover(browser, sid, args.cover)
            await browser.screenshot("/tmp/douyin_article_step3_cover.png", sid=sid)

        # 6. 发布
        if args.dry_run:
            log("Dry run - skipping publish")
            await browser.screenshot("/tmp/douyin_article_dryrun.png", sid=sid)
            print(json.dumps({"status": "dry_run", "screenshots": [
                "/tmp/douyin_article_step0.png",
                "/tmp/douyin_article_step1.png",
                "/tmp/douyin_article_step2.png",
            ]}, ensure_ascii=False))
            return

        await click_publish(browser, sid)
        await asyncio.sleep(3)
        await browser.screenshot("/tmp/douyin_article_step4.png", sid=sid)

        success = await verify_publish(browser, sid)
        print(json.dumps({
            "status": "published" if success else "uncertain",
            "title": args.title,
        }, ensure_ascii=False))


asyncio.run(main())
