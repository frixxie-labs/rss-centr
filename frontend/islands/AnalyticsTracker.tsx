import { useEffect } from "preact/hooks";

import { trackPageView } from "../analytics.ts";

interface AnalyticsTrackerProps {
  path: string;
}

export default function AnalyticsTracker({ path }: AnalyticsTrackerProps) {
  useEffect(() => {
    void trackPageView(path);
  }, [path]);

  return null;
}
