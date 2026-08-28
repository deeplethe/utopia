-- 应用此前用数据库 owner（superuser）连库。那个身份能 DROP TRIGGER、改任何表——
-- 也就是说 0026 给台账加的不可变触发器，对拿到应用连接串的人形同虚设：
-- DISABLE TRIGGER、改记录、ENABLE TRIGGER，三条 SQL，事后毫无痕迹。
--
-- 受限角色把应用降到它真正需要的权限：业务表随便读写，台账只能写和读。
-- 于是同样那三条 SQL 第一条就报 must be owner of table。
--
-- 这一层挡的是应用自身的 bug、SQL 注入继承的权限、以及泄漏出去的连接串
-- （日志、备份、误提交的 .env）。它挡不住能登进服务器的人——容器内 psql
-- 是 trust 认证，免密即 superuser。那个层面属于服务器访问控制，不属于这里。
--
-- 整段在角色不存在时跳过：既有部署不建角色、不改连接串即可照常运行。
-- ALTER DEFAULT PRIVILEGES 也必须包在里面——它在 DO 块外会因角色不存在而
-- 报错中断迁移，那将让每一个既有部署升级即失败。
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'utopia_app') THEN
        RAISE NOTICE 'utopia_app 角色不存在，跳过受限权限配置（应用继续以当前身份运行）';
        RETURN;
    END IF;

    GRANT USAGE ON SCHEMA public TO utopia_app;
    GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO utopia_app;
    GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO utopia_app;
    GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA public TO utopia_app;

    -- 台账：写得进、读得到，改不动、删不掉
    REVOKE UPDATE, DELETE, TRUNCATE ON audit_events FROM utopia_app;

    -- 往后迁移新建的表/序列/函数自动授权，否则每加一张表应用就撞权限错误。
    -- PL/pgSQL 不接受这条 utility 命令的直写形式，走 EXECUTE。
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
         || 'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO utopia_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
         || 'GRANT USAGE, SELECT ON SEQUENCES TO utopia_app';
    EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA public '
         || 'GRANT EXECUTE ON FUNCTIONS TO utopia_app';

    RAISE NOTICE 'utopia_app 受限权限已配置';
END $$;
