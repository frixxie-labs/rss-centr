DELETE FROM feeds
WHERE url IN (
    'https://www.nrk.no/nyheter/siste.rss',
    'https://rss.kode24.no/',
    'https://www.adressa.no/rss',
    'https://www.tek.no/api/rss/rss2/medium/collections',
    'https://blog.rust-lang.org/feed.xml',
    'https://blog.rust-lang.org/inside-rust/feed.xml',
    'https://this-week-in-rust.org/rss.xml',
    'https://planet.rust-lang.org/rss20.xml',
    'https://hnrss.org/frontpage',
    'https://github.blog/feed/'
);
