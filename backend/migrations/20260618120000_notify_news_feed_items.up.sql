CREATE OR REPLACE FUNCTION notify_news_feed_item_inserted()
RETURNS TRIGGER AS $$
BEGIN
    PERFORM pg_notify('news_feed_items', NEW.id::TEXT);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER feed_items_notify_insert
AFTER INSERT ON feed_items
FOR EACH ROW
EXECUTE FUNCTION notify_news_feed_item_inserted();
