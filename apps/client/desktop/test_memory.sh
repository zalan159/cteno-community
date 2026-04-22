#!/bin/bash
# Memory System End-to-End Test
# Run this after starting the Tauri app

API_BASE="http://127.0.0.1:19198"

echo "=== Memory System E2E Test ==="
echo ""

# Test 1: Check server
echo "1. Checking server status..."
SKILLS=$(curl -s "$API_BASE/skills" | jq -r '.success')
if [ "$SKILLS" = "true" ]; then
  echo "   ✅ Server is running"
else
  echo "   ❌ Server not available"
  exit 1
fi

# Test 2: Write memory via zsh skill (since memory commands are Tauri-only)
echo ""
echo "2. Writing test memory file..."
WRITE_RESULT=$(curl -s -X POST "$API_BASE/skills/zsh/execute" \
  -H "Content-Type: application/json" \
  -d '{"params":{"command":"mkdir -p ~/Library/Application\\ Support/com.cteno.dev/workspace && echo \"# 测试记忆\n\n## 测试内容\n- 这是一个测试\n- 用于验证记忆系统\" > ~/Library/Application\\ Support/com.cteno.dev/workspace/TEST.md"}}' | jq -r '.success')
if [ "$WRITE_RESULT" = "true" ]; then
  echo "   ✅ Test file written"
else
  echo "   ❌ Failed to write test file"
fi

# Test 3: Read memory file
echo ""
echo "3. Reading test memory file..."
READ_RESULT=$(curl -s -X POST "$API_BASE/skills/zsh/execute" \
  -H "Content-Type: application/json" \
  -d '{"params":{"command":"cat ~/Library/Application\\ Support/com.cteno.dev/workspace/TEST.md"}}' | jq -r '.data.result.stdout')
if [[ "$READ_RESULT" == *"测试记忆"* ]]; then
  echo "   ✅ File content verified"
else
  echo "   ❌ File content mismatch"
  echo "   Got: $READ_RESULT"
fi

# Test 4: List models
echo ""
echo "4. Checking embedding model..."
MODELS=$(curl -s -X POST "$API_BASE/skills/zsh/execute" \
  -H "Content-Type: application/json" \
  -d '{"params":{"command":"ls ~/Library/Application\\ Support/com.cteno.dev/models/*.gguf 2>/dev/null | head -1"}}' | jq -r '.data.result.stdout')
if [[ -n "$MODELS" && "$MODELS" != "null" ]]; then
  echo "   ✅ Model found: $(basename "$MODELS")"
else
  echo "   ⚠️ No embedding model found"
  echo "   Download with: curl -L -o ~/Library/Application\\ Support/com.cteno.dev/models/nomic-embed-text-v1.5.Q4_K_M.gguf https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF/resolve/main/nomic-embed-text-v1.5.Q4_K_M.gguf"
fi

# Test 5: Check database
echo ""
echo "5. Checking database..."
DB_EXISTS=$(curl -s -X POST "$API_BASE/skills/zsh/execute" \
  -H "Content-Type: application/json" \
  -d '{"params":{"command":"ls ~/Library/Application\\ Support/com.cteno.dev/db/*.db 2>/dev/null | wc -l"}}' | jq -r '.data.result.stdout' | tr -d ' \n')
if [ "$DB_EXISTS" -gt "0" ]; then
  echo "   ✅ Database files found"
else
  echo "   ⚠️ Database not initialized (start Tauri app to initialize)"
fi

echo ""
echo "=== Test Complete ==="
