#!/bin/bash

# Test Read Tool Integration
#
# Tests the ReadExecutor with various file types and scenarios

set -e

API_BASE="http://localhost:19198"

echo "=== Read Tool Integration Test ==="
echo

# Check if server is running
if ! curl -s "${API_BASE}/health" > /dev/null; then
    echo "❌ Server is not running at ${API_BASE}"
    echo "Please start the server with: cargo run --release"
    exit 1
fi

echo "✅ Server is running"
echo

# Create test files
TEST_DIR="/tmp/cteno_read_test_$$"
mkdir -p "$TEST_DIR"

echo "Setting up test files in $TEST_DIR..."
echo

# Test 1: Simple text file
echo "Test 1: Read simple text file"
cat > "$TEST_DIR/simple.txt" <<EOF
Line 1
Line 2
Line 3
EOF

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/simple.txt\"
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Simple text file read passed"
    echo "$result" | jq -r '.result' | head -5
else
    echo "❌ Simple text file read failed"
    echo "$result" | jq '.'
fi
echo

# Test 2: File with pagination
echo "Test 2: Read large file with pagination"
for i in {1..100}; do
    echo "Line $i" >> "$TEST_DIR/large.txt"
done

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/large.txt\",
            \"offset\": 10,
            \"limit\": 5
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Pagination test passed"
    content=$(echo "$result" | jq -r '.result')
    if echo "$content" | grep -q "Line 11" && echo "$content" | grep -q "Line 15"; then
        echo "  ✓ Correct lines returned (11-15)"
    else
        echo "  ✗ Incorrect pagination result"
    fi
else
    echo "❌ Pagination test failed"
    echo "$result" | jq '.'
fi
echo

# Test 3: Truncation of large file
echo "Test 3: Read large file (truncation)"
for i in {1..5000}; do
    echo "Line $i" >> "$TEST_DIR/verylarge.txt"
done

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/verylarge.txt\"
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Large file truncation test passed"
    content=$(echo "$result" | jq -r '.result')
    if echo "$content" | grep -q "\[TRUNCATED\]"; then
        echo "  ✓ Truncation message present"
        echo "$content" | grep "Showing lines"
    else
        echo "  ✗ Truncation message missing"
    fi
else
    echo "❌ Large file truncation test failed"
    echo "$result" | jq '.'
fi
echo

# Test 4: Long line truncation
echo "Test 4: Long line truncation"
python3 -c "print('a' * 3000)" > "$TEST_DIR/longline.txt"

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/longline.txt\"
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Long line truncation test passed"
    content=$(echo "$result" | jq -r '.result')
    if echo "$content" | grep -q "\[truncated\]"; then
        echo "  ✓ Long line truncated correctly"
    else
        echo "  ✗ Long line not truncated"
    fi
else
    echo "❌ Long line truncation test failed"
    echo "$result" | jq '.'
fi
echo

# Test 5: File not found
echo "Test 5: File not found error"
result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "file_path": "/nonexistent/file.txt"
        }
    }')

if echo "$result" | jq -e '.success == false' > /dev/null 2>&1; then
    echo "✅ File not found error handled correctly"
    echo "  Error: $(echo "$result" | jq -r '.error')"
else
    echo "❌ File not found test failed"
    echo "$result" | jq '.'
fi
echo

# Test 6: Binary file detection
echo "Test 6: Binary file detection"
# Create a simple binary file (PNG header)
printf '\x89PNG\r\n\x1a\n' > "$TEST_DIR/test.png"

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/test.png\"
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Binary file detection passed"
    content=$(echo "$result" | jq -r '.result')
    if echo "$content" | grep -q "Image file"; then
        echo "  ✓ Recognized as image file"
    fi
else
    echo "❌ Binary file detection failed"
    echo "$result" | jq '.'
fi
echo

# Test 7: UTF-8 BOM handling
echo "Test 7: UTF-8 BOM handling"
printf '\xEF\xBB\xBFHello, UTF-8 BOM!' > "$TEST_DIR/utf8bom.txt"

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d "{
        \"params\": {
            \"file_path\": \"$TEST_DIR/utf8bom.txt\"
        }
    }")

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ UTF-8 BOM handling passed"
    content=$(echo "$result" | jq -r '.result')
    echo "  Content: $content"
else
    echo "❌ UTF-8 BOM handling failed"
    echo "$result" | jq '.'
fi
echo

# Test 8: Path traversal protection
echo "Test 8: Path traversal protection"
result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "file_path": "../../etc/passwd"
        }
    }')

if echo "$result" | jq -e '.success == false' > /dev/null 2>&1; then
    echo "✅ Path traversal blocked correctly"
    echo "  Error: $(echo "$result" | jq -r '.error')"
else
    echo "❌ Path traversal protection failed"
    echo "$result" | jq '.'
fi
echo

# Test 9: Tilde expansion
echo "Test 9: Tilde expansion"
cat > ~/cteno_test_tilde.txt <<EOF
Tilde expansion test
EOF

result=$(curl -s -X POST "${API_BASE}/tools/read/execute" \
    -H 'Content-Type: application/json' \
    -d '{
        "params": {
            "file_path": "~/cteno_test_tilde.txt"
        }
    }')

if echo "$result" | jq -e '.success' > /dev/null 2>&1; then
    echo "✅ Tilde expansion test passed"
    content=$(echo "$result" | jq -r '.result')
    if echo "$content" | grep -q "Tilde expansion test"; then
        echo "  ✓ File content read correctly"
    fi
else
    echo "❌ Tilde expansion test failed"
    echo "$result" | jq '.'
fi
rm -f ~/cteno_test_tilde.txt
echo

# Cleanup
echo "Cleaning up test files..."
rm -rf "$TEST_DIR"

echo "=== All Tests Complete ==="
