package com.mdhaarith.patch;

/**
 * Turns one app's scroll events into short videos swiped; feeds never count. Pure logic, see ScrollCounterTest.
 *
 * Two kinds of video pager:
 * - RecyclerView/ViewPager2 (Instagram Reels, YouTube Shorts): events carry the first visible item index
 *   (fromIndex) and the moved pixels (dy). A video is a new index reached after about a full screen of scrolling;
 *   feed posts are shorter.
 * - Compose and ViewPager (X, TikTok): events carry only the absolute scrollY. A pager swipe moves whole pages and
 *   starts and ends on page boundaries; feed flings land anywhere. The first video teaches the page size, after
 *   which only moves of whole pages count.
 * In both, only videos past the furthest one reached count, so swiping back and forth doesn't recount.
 */
final class ScrollCounter {
    static final long SWIPE_MS = 500;    // events further apart than this belong to separate swipes
    static final int MIN_PAGE = 100;     // smaller moves are jitter, not a page
    static final float PAGE = 0.85f;     // a video page scrolls at least this share of the screen
    static final int MAX_JUMP = 10;      // bigger index jumps = switched lists, not scrolling
    static final int MAX_PAGES = 5;      // more pages in one swipe = switched lists, not a fling

    long videos; // since the last flush to storage

    private int lastY = -1, swipeStart = -1, maxY = -1, page;
    private int pending; // length of a swipe from the top that may be a page, until another swipe confirms it
    private int lastIndex = -1, maxIndex = -1;
    private long pxSinceItem, lastTime;

    /**
     * Whether an event is a vertical scroll worth counting. Horizontal lists (story trays, image carousels)
     * would scramble the vertical list's position. dx, dy = raw deltas (-1 = not set);
     * maxScrollY = vertical range (0 or -1 for lists that can't scroll vertically).
     */
    static boolean vertical(int fromIndex, int maxScrollY, int dx, int dy) {
        if (dx != 0 && dx != -1 && (dy == 0 || dy == -1)) return false; // moved sideways only
        return fromIndex >= 0 || maxScrollY > 0;
    }

    /**
     * fromIndex = first visible item or -1, scrollY = absolute position or -1, dy = pixels moved
     * (0 or -1 when unknown), screenH = screen height px, now = monotonic ms.
     */
    void onScroll(int fromIndex, int scrollY, int dy, int screenH, long now) {
        if (lastTime > 0 && now - lastTime > SWIPE_MS) endSwipe();
        lastTime = now;
        if (fromIndex >= 0) {
            onIndex(fromIndex, dy, screenH);
            return;
        }
        if (scrollY < 0) return;
        if (lastY >= 0 && Math.abs(scrollY - lastY) > 3 * screenH) {
            swipeStart = -1; // switched lists: positions start over
            maxY = -1;
            lastY = -1;
        }
        if (swipeStart < 0) swipeStart = lastY >= 0 ? lastY : scrollY;
        if (maxY < 0) maxY = swipeStart;
        lastY = scrollY;
    }

    private void onIndex(int index, int dy, int screenH) {
        if (dy > 0) pxSinceItem += dy;
        if (lastIndex < 0 || Math.abs(index - lastIndex) > MAX_JUMP) {
            maxIndex = index; // first event, or switched lists
        } else if (index > maxIndex) {
            int n = index - maxIndex;
            // Unknown distance (dy missing) never counts: a feed and a pager look the same then.
            if (pxSinceItem / n >= screenH * PAGE) videos += n;
            maxIndex = index;
        }
        if (index != lastIndex) pxSinceItem = 0;
        lastIndex = index;
    }

    /** Call once scrolling has been quiet for SWIPE_MS; safe to call twice. */
    void endSwipe() {
        int start = swipeStart, end = lastY;
        swipeStart = -1;
        if (start < 0 || end <= maxY) return; // no swipe, or nothing new past the furthest video
        int d = end - start;
        if (d < MIN_PAGE) return;
        boolean aligned = start % d == 0 && end % d == 0; // a single page from boundary to boundary
        int from = Math.max(start, maxY);
        if (page == 0 || (d % page != 0 && aligned)) {
            if (!aligned) return; // a feed fling
            // 0 is a boundary for every size, so any feed swipe from the top looks aligned: wait for a second one.
            if (start == 0 && pending != d) {
                pending = d;
                return;
            }
            if (pending == d) from = maxY; // the swipe from the top was a real page too
            page = d; // learn, or relearn if a lucky feed fling taught a wrong size
        }
        if (d % page != 0 || start % page != 0 || d / page > MAX_PAGES) return;
        videos += (end - from) / page;
        maxY = end;
    }
}
