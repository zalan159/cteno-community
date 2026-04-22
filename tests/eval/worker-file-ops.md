# Worker 文件操作测试

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-test
- max-turns: 15

## setup
```bash
mkdir -p /tmp/cteno-test
echo 'def hello():\n    print("hello world")\n\nhello()' > /tmp/cteno-test/hello.py
```

## cases

### [pending] 路径含空格时 edit 是否正常工作
- **message**: "修改 '/tmp/cteno-test/hello.py'，把 print 改成 return，函数名改成 greet"
- **expect**: 用 edit 工具（非 write 覆盖）完成两处替换，文件语法正确
- **anti-pattern**: 用 write 整体覆盖文件；用 shell sed 替换
- **severity**: high

### [pending] 不存在的文件路径下 Agent 的恢复行为
- **message**: "读取 /tmp/cteno-test/nonexistent/deep/path/config.json 并总结内容"
- **expect**: Agent 识别文件不存在，向用户说明，不胡编内容
- **anti-pattern**: 幻觉编造文件内容；崩溃无回复
- **severity**: high

### [pending] 矛盾指令处理
- **message**: "删除 /tmp/cteno-test/hello.py，然后读取它的内容并打印到终端"
- **expect**: Agent 识别矛盾（删除后无法读取），选择合理策略（先读后删，或询问）
- **anti-pattern**: 盲目按顺序执行导致报错后无恢复
- **severity**: medium

### [pending] 大量连续 edit 的稳定性
- **message**: "在 /tmp/cteno-test/hello.py 中：1) 添加 import os 2) 添加 import sys 3) 把 hello 改名为 main 4) 添加 if __name__ == '__main__': main() 5) 添加文件头注释"
- **expect**: 所有 5 项修改完成，文件可执行，edit 调用次数 <= 6
- **anti-pattern**: 中途放弃用 write 覆盖；edit 失败后不重试
- **severity**: medium
