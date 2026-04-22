#!/bin/bash

# Test Tool Executors Integration
#
# This script tests the new Tool executor system

set -e

API_BASE="http://localhost:19198"

echo "=== Tool Executors Integration Test ==="
echo

# Check if server is running
if ! curl -s "${API_BASE}/health" > /dev/null; then
    echo "❌ Server is not running at ${API_BASE}"
    echo "Please start the server with: cargo run --release"
    exit 1
fi

echo "✅ Server is running"
echo

# Test 1: List all tools
echo "Test 1: List all tools"
tools=$(curl -s "${API_BASE}/tools")
echo "$tools" | jq -r '.[] | "  - \(.id): \(.name)"'
echo

# Test 2: Shell executor
echo "Test 2: Shell executor"
result=$(curl -s -X POST "${API_BASE}/tools/shell/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "command": "echo \"Hello from Shell Executor!\""
        }
    }')

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Shell executor test passed"
    echo "$result" | jq -r '.result' | sed 's/^/  /'
else
    echo "❌ Shell executor test failed"
    echo "$result" | jq '.'
fi
echo

# Test 3: File executor - write and read
echo "Test 3: File executor - write and read"
test_file="/tmp/cteno_test_$(date +%s).txt"
test_content="Hello from File Executor!"

# Write
write_result=$(curl -s -X POST "${API_BASE}/tools/file/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"operation\": \"write\",
            \"path\": \"$test_file\",
            \"content\": \"$test_content\"
        }
    }")

if echo "$write_result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ File write test passed"
else
    echo "❌ File write test failed"
    echo "$write_result" | jq '.'
fi

# Read
read_result=$(curl -s -X POST "${API_BASE}/tools/file/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"operation\": \"read\",
            \"path\": \"$test_file\"
        }
    }")

if echo "$read_result" | jq -e '.success' > /dev/null 2>&1; then
    content=$(echo "$read_result" | jq -r '.result')
    if [ "$content" = "$test_content" ]; then
        echo "✅ File read test passed"
        echo "  Content: $content"
    else
        echo "❌ File read test failed - content mismatch"
        echo "  Expected: $test_content"
        echo "  Got: $content"
    fi
else
    echo "❌ File read test failed"
    echo "$read_result" | jq '.'
fi

# Cleanup
rm -f "$test_file"
echo

# Test 4: File executor - list directory
echo "Test 4: File executor - list directory"
list_result=$(curl -s -X POST "${API_BASE}/tools/file/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "operation": "list",
            "path": "/tmp"
        }
    }')

if echo "$list_result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ File list test passed"
    echo "$list_result" | jq -r '.result' | head -10 | sed 's/^/  /'
else
    echo "❌ File list test failed"
    echo "$list_result" | jq '.'
fi
echo

# Test 5: Edit executor
echo "Test 5: Edit executor"
test_edit_file="/tmp/cteno_edit_test_$(date +%s).txt"
echo -e "Line 1\nLine 2\nLine 3" > "$test_edit_file"

edit_result=$(curl -s -X POST "${API_BASE}/tools/edit/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$test_edit_file\",
            \"old_string\": \"Line 2\",
            \"new_string\": \"Modified Line 2\"
        }
    }")

if echo "$edit_result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Edit executor test passed"
    cat "$test_edit_file" | sed 's/^/  /'
else
    echo "❌ Edit executor test failed"
    echo "$edit_result" | jq '.'
fi

rm -f "$test_edit_file"
echo

# Test 6: WebSearch executor
echo "Test 6: WebSearch executor"
search_result=$(curl -s -X POST "${API_BASE}/tools/websearch/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "query": "Rust programming language",
            "max_results": 3
        }
    }')

if echo "$search_result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ WebSearch executor test passed"
    echo "$search_result" | jq -r '.result' | head -5 | sed 's/^/  /'
else
    echo "❌ WebSearch executor test failed"
    echo "$search_result" | jq '.'
fi
echo

echo "=== All Tests Complete ==="
