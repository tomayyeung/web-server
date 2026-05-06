-- Insert test users
-- Passwords are hashed with bcrypt. For testing, use:
-- alice: password123
-- bob: password123
INSERT OR IGNORE INTO users (id, username, password_hash) VALUES
    ('550e8400-e29b-41d4-a716-446655440000', 'alice', '$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/LewY5YmMxSUmGEJiq'),
    ('550e8400-e29b-41d4-a716-446655440001', 'bob', '$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/LewY5YmMxSUmGEJiq');

INSERT OR IGNORE INTO bookmark (user_id, url, title) VALUES
    ('550e8400-e29b-41d4-a716-446655440000', 'https://doc.rust-lang.org/book/', 'The Rust Programming Language'),
    ('550e8400-e29b-41d4-a716-446655440000', 'https://www.sqlite.org/', 'SQLite Home Page'),
    ('550e8400-e29b-41d4-a716-446655440001', 'https://en.wikipedia.org/wiki/SQL', 'SQL - Wikipedia');

INSERT OR IGNORE INTO tag (name) VALUES
    ('rust'),
    ('programming'),
    ('learning'),
    ('sqlite'),
    ('database'),
    ('sql'),
    ('reference');

INSERT OR IGNORE INTO bookmark_tag (bookmark_id, tag_id) VALUES
    -- The Rust Book: rust, programming, learning
    ((SELECT id FROM bookmark WHERE url = 'https://doc.rust-lang.org/book/'),
     (SELECT id FROM tag WHERE name = 'rust')),
    ((SELECT id FROM bookmark WHERE url = 'https://doc.rust-lang.org/book/'),
     (SELECT id FROM tag WHERE name = 'programming')),
    ((SELECT id FROM bookmark WHERE url = 'https://doc.rust-lang.org/book/'),
     (SELECT id FROM tag WHERE name = 'learning')),
    -- SQLite Home Page: sqlite, database
    ((SELECT id FROM bookmark WHERE url = 'https://www.sqlite.org/'),
     (SELECT id FROM tag WHERE name = 'sqlite')),
    ((SELECT id FROM bookmark WHERE url = 'https://www.sqlite.org/'),
     (SELECT id FROM tag WHERE name = 'database')),
    -- SQL - Wikipedia: sql, database, reference
    ((SELECT id FROM bookmark WHERE url = 'https://en.wikipedia.org/wiki/SQL'),
     (SELECT id FROM tag WHERE name = 'sql')),
    ((SELECT id FROM bookmark WHERE url = 'https://en.wikipedia.org/wiki/SQL'),
     (SELECT id FROM tag WHERE name = 'database')),
    ((SELECT id FROM bookmark WHERE url = 'https://en.wikipedia.org/wiki/SQL'),
     (SELECT id FROM tag WHERE name = 'reference'));
