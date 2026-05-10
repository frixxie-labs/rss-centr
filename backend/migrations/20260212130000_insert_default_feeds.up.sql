-- Seed a small set of default RSS feeds.
--
-- NOTE: Use ON CONFLICT DO NOTHING so this migration is safe to re-run
-- in dev/test environments.

INSERT INTO feeds (url)
VALUES
    ('https://www.nrk.no/nyheter/siste.rss'),
    ('https://rss.kode24.no/'),
    ('https://www.adressa.no/rss'),
    ('https://www.tek.no/api/rss/rss2/medium/collections'),
    ('https://blog.rust-lang.org/feed.xml'),
    ('https://blog.rust-lang.org/inside-rust/feed.xml'),
    ('https://this-week-in-rust.org/rss.xml'),
    ('https://planet.rust-lang.org/rss20.xml'),
    ('https://hnrss.org/frontpage'),
    ('https://github.blog/feed/'),
    ('https://feeds.zencastr.com/f/oSn1i316.rss'),
    ('https://letscast.fm/podcasts/rust-in-production-82281512/feed'),
    ('https://feeds.simplecast.com/7y1CbAbN'),
    ('https://e24.no/rss2/'),
    ('https://www.jeffgeerling.com/blog.xml'),
    ('https://services.dn.no/api/feed/rss/'),
    ('https://www.tv2.no/rss/nyheter'),
    ('https://softskills.audio/feed.xml')
ON CONFLICT (url) DO NOTHING;
