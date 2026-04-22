# Host Bootstrap 宿主下沉回归

验证 P1 重构（`refactor_p1_host_plan.md`）之后宿主初始化、daemon 生命周期、身份解析的关键边界行为。

## meta
- kind: worker
- profile: proxy-deepseek-reasoner
- workdir: /tmp/cteno-host-bootstrap
- max-turns: 12

## setup

```bash
mkdir -p /tmp/cteno-host-bootstrap
rm -rf /tmp/cteno-host-bootstrap/.agents
# 注意：不要删 $HOME/Library/Application Support/Cteno Agentd，若有真用户数据会被清掉
```

## cases

### [pending] cteno-agentd 在 app_data_dir 权限只读时给出明确错误
- **message**: 模拟把 app_data_dir 置为只读（`chmod 555`）后启动 cteno-agentd，观察输出
- **expect**: 返回明确的 "Failed to create app data dir" 或同语义错误，进程退出码非 0，不 panic，不留 daemon.lock
- **anti-pattern**: panic/abort；silent 启动但 RPC 不可用；误导性错误提示；lock 文件残留阻塞下次启动
- **severity**: high

### [pending] cteno-agentd 检测到 Tauri release 目录已存在但 config.json 缺失不阻塞启动
- **message**: Tauri release app_data_dir 存在但没有 `config.json`，启动 cteno-agentd
- **expect**: `seed_headless_identity_from_tauri` 跳过不存在的 config.json，daemon 正常启动，identity paths 指向 headless 目录
- **anti-pattern**: 拷贝不存在的文件导致 I/O 报错；identity 错误指向 Tauri 目录造成双写
- **severity**: high

### [pending] managed-mode 与 community-mode 切换后 machine_id 保持一致
- **message**: 先在 community（`--no-default-features`）构建的 daemon 下启动生成 machine_id，读取 `machine_id.txt`；再切 commercial 构建启动一次；再切回 community；对比三次读数
- **expect**: machine_id 三次读数完全一致；`daemon_state.rs::machine_id_path` 与 `paths.rs::machine_id_path` 解析一致
- **anti-pattern**: feature gate 切换后 machine_id 被重置；`HappyMachineIdProvider` 在 community build 找不到路径
- **severity**: high

### [pending] HostMachineRuntime 在 spawner 返回错误时不绑定 socket
- **message**: 人为注入一个 `HostMachineSpawner` impl 让 `start_machine_host` 早期 `return`，观察 `local_rpc_server::start` 是否执行、socket 是否绑定
- **expect**: 记录 error，不绑定 socket，daemon 流程退出；没有 daemon.lock 残留
- **anti-pattern**: socket 仍然 bind；lock 文件残留阻塞下次启动；panic 冒泡
- **severity**: medium

### [pending] resolve_tauri_paths_from 与 resolve_headless_paths 对同一 app_data_dir 产出等价 HostPaths
- **message**: 给定同一个 app_data_dir，分别走 `resolve_tauri_paths_from`（传入伪造 resolved_dirs）与 `resolve_headless_paths_with_manifest`，对比产出的 `HostPaths.identity.app_data_dir` / `db_path` / `builtin_skills_dir` / `config_path` 五项
- **expect**: Tauri 路径走 AppHandle bundled resources，headless 路径走 manifest_dir 查找 bundled 资源；两者 app_data_dir/db_path/config_path 相同，builtin_* 路径可能不同但均存在
- **anti-pattern**: Tauri 路径误指向 headless 的 manifest_dir；headless 路径空串或 panic
- **severity**: medium
```
