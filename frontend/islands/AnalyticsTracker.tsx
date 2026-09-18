import { useEffect } from "preact/hooks";

import { trackPageView } from "../analytics.ts";

export default function AnalyticsTracker() {
  useEffect(() => {
    void trackPageView();
  }, []);

  return null;
}
