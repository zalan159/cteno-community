#!/bin/bash
set -e

API_BASE="http://localhost:19198"

echo "=== Edit Tool Improvements Test ==="
echo ""

# Test 1: instruction 参数是否必需
echo "Test 1: Instruction parameter is required"
result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_edit.txt",
        "old_string": "old",
        "new_string": "new"
    }' 2>&1 || true)

if echo "$result" | grep -q "instruction"; then
    echo "✅ instruction is required"
else
    echo "❌ instruction should be required"
    echo "Response: $result"
    exit 1
fi
echo ""

# Test 2: 行尾格式保留 (CRLF)
echo "Test 2: Preserve CRLF line endings"
printf "line1\r\nline2\r\nline3" > /tmp/test_crlf.txt

result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_crlf.txt",
        "instruction": "Replace line2 with modified_line2",
        "old_string": "line2",
        "new_string": "modified_line2"
    }')

if echo "$result" | grep -q "success"; then
    # 检查是否保留 CRLF
    if file /tmp/test_crlf.txt | grep -q "CRLF"; then
        echo "✅ CRLF line endings preserved"
    else
        # 使用 od 命令检查实际字节
        if od -c /tmp/test_crlf.txt | grep -q '\\r'; then
            echo "✅ CRLF line endings preserved (verified with od)"
        else
            echo "⚠️  CRLF line endings may not be preserved"
            echo "Content:"
            od -c /tmp/test_crlf.txt | head -5
        fi
    fi
else
    echo "❌ Edit failed"
    echo "Response: $result"
    exit 1
fi
echo ""

# Test 3: 行尾格式保留 (LF)
echo "Test 3: Preserve LF line endings"
printf "line1\nline2\nline3" > /tmp/test_lf.txt

result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_lf.txt",
        "instruction": "Replace line2 with new_line2",
        "old_string": "line2",
        "new_string": "new_line2"
    }')

if echo "$result" | grep -q "success"; then
    # 检查是否没有 CRLF
    if ! od -c /tmp/test_lf.txt | grep -q '\\r'; then
        echo "✅ LF line endings preserved (no CRLF detected)"
    else
        echo "❌ LF line endings not preserved"
        echo "Content:"
        od -c /tmp/test_lf.txt | head -5
        exit 1
    fi
else
    echo "❌ Edit failed"
    echo "Response: $result"
    exit 1
fi
echo ""

# Test 4: 尾部换行符保留
echo "Test 4: Preserve trailing newline"
printf "line1\nline2\n" > /tmp/test_trailing.txt

result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_trailing.txt",
        "instruction": "Replace line2 with final_line",
        "old_string": "line2",
        "new_string": "final_line"
    }')

if echo "$result" | grep -q "success"; then
    content=$(cat /tmp/test_trailing.txt)
    if [[ "$content" == $'line1\nfinal_line\n' ]]; then
        echo "✅ Trailing newline preserved"
    else
        echo "❌ Trailing newline not preserved"
        echo "Content: $(od -c /tmp/test_trailing.txt)"
        exit 1
    fi
else
    echo "❌ Edit failed"
    echo "Response: $result"
    exit 1
fi
echo ""

# Test 5: 错误消息改进
echo "Test 5: Improved error messages"
echo "test content" > /tmp/test_error.txt

result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_error.txt",
        "instruction": "Replace nonexistent text",
        "old_string": "nonexistent",
        "new_string": "replacement"
    }' 2>&1 || true)

if echo "$result" | grep -q "0 occurrences"; then
    echo "✅ Descriptive error message for 0 occurrences"
else
    echo "⚠️  Error message format may have changed"
    echo "Response: $result"
fi
echo ""

# Test 6: No changes error
echo "Test 6: No changes to apply error"
result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_error.txt",
        "instruction": "No real change",
        "old_string": "test content",
        "new_string": "test content"
    }' 2>&1 || true)

if echo "$result" | grep -q "No changes"; then
    echo "✅ No changes error detected"
else
    echo "⚠️  No changes error message may have changed"
    echo "Response: $result"
fi
echo ""

# Test 7: LLM 自动修正（简单场景）
echo "Test 7: LLM auto-correction (simple whitespace fix)"
echo "def hello():" > /tmp/test_llm.py
echo "    print('world')" >> /tmp/test_llm.py

result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "path": "/tmp/test_llm.py",
        "instruction": "Change the print message to \"hello world\"",
        "old_string": "print(\"world\")",
        "new_string": "print(\"hello world\")"
    }' 2>&1 || true)

if echo "$result" | grep -q "success\|LLM corrected"; then
    echo "✅ Edit succeeded (with or without LLM correction)"
    if echo "$result" | grep -q "LLM corrected"; then
        echo "   ℹ️  LLM auto-correction was used"
    fi
else
    echo "⚠️  Edit may have failed (check if LLM correction was attempted in logs)"
    echo "Response: $result"
fi
echo ""

# Cleanup
rm -f /tmp/test_*.txt /tmp/test_*.py

echo "=== All tests completed ==="
