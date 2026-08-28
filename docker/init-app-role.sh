#!/bin/bash
# 创建应用运行时使用的受限角色。仅在数据目录为空的首次初始化时执行
# （Postgres 官方镜像的 docker-entrypoint-initdb.d 约定），既有部署不受影响。
#
# 权限不在这里给——由迁移 0031 授予，那样每次升级都能把新表补进来。
# 这里只负责角色本身存在，且拥有一个可登录的口令。
set -euo pipefail

APP_PASSWORD="${UTOPIA_APP_DB_PASSWORD:-}"
if [ -z "$APP_PASSWORD" ]; then
    echo "未设置 UTOPIA_APP_DB_PASSWORD，跳过受限角色创建；应用将以 owner 身份运行。" >&2
    exit 0
fi

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<SQL
DO \$\$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'utopia_app') THEN
        CREATE ROLE utopia_app LOGIN PASSWORD '${APP_PASSWORD}';
        RAISE NOTICE '已创建受限角色 utopia_app';
    END IF;
END
\$\$;
GRANT CONNECT ON DATABASE "$POSTGRES_DB" TO utopia_app;
SQL
