SELECT l.pid AS "pid!"
FROM pg_catalog.pg_locks AS l
WHERE NOT l.granted
ORDER BY l.pid
