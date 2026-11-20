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
 * The video a pager opens on (Reels tab, a reel tapped in the feed) counts with the first swipe that proves the
 * list is a pager; one opened and left without a swipe can't be told from a tall feed post, so it doesn't count.
 */
final class ScrollCounter {
    static final long SWIPE_MS = 500;    // events further apart than this belong to separate swipes
    static final int MIN_PAGE = 100;     // smaller moves are jitter, not a page
    static final float PAGE = 0.85f;     // a video page scrolls at least this share of the screen
    static final int MAX_JUMP = 10;      // bigger index jumps = switched lists, not scrolling
    static final int MAX_PAGES = 5;      // more pages in one swipe = switched lists, not a fling

    /** Android can report another foreground app while X keeps its viewer on the same page. */
    static final class AppSessions {
        private ScrollCounter x;

        ScrollCounter enter(boolean enteringX) {
            if (!enteringX) return new ScrollCounter();
            if (x == null || !x.inViewer()) x = new ScrollCounter();
            return x;
        }
    }

    private boolean viewer;
    private int viewerHeight, viewerOffset, viewerIndex, furthestViewerIndex;
    private int viewerStride;
    private boolean sawAbsoluteOffset;
    private int firstPagerY = -1, openingPage = -1;

    boolean hasPixelPager() {
        return firstPagerY >= 0;
    }

    /** X reports absolute pixels even when the tapped video opens several pages into the viewer. */
    boolean onPagerPosition(int y, int range) {
        if (!viewer || y < 0 || range < viewerHeight || range < y || range % viewerHeight != 0)
            return false;
        if (firstPagerY < 0) firstPagerY = y;
        if (openingPage < 0) {
            int page = firstPagerY / viewerHeight;
            if (firstPagerY % viewerHeight == 0 || y > firstPagerY) openingPage = page;
            else if (y < firstPagerY) openingPage = page + 1;
        }
        viewerOffset = y;
        return true;
    }

    // A reply composer can be exposed as an editor even when the sheet has no dismiss action.
    static boolean isBottomEditor(int width, int top, int bottom, int screenWidth, int screenHeight) {
        return width >= screenWidth / 2 && top >= screenHeight * 0.7f && bottom >= screenHeight * 0.85f;
    }

    boolean inViewer() {
        return viewer;
    }

    /** A full-screen vertical pager appeared or disappeared inside the same X window. */
    void setViewer(int height) {
        boolean visible = height > 0;
        if (visible == viewer) {
            viewerHeight = height;
            return;
        }
        endSwipe();
        viewerHeight = height;
        viewer = visible;
        lastY = visible ? 0 : -1;
        maxY = lastY;
        viewerOffset = viewerIndex = furthestViewerIndex = viewerStride = 0;
        sawAbsoluteOffset = false;
        firstPagerY = openingPage = -1;
        page = pending = 0;
        lastIndex = maxIndex = visible ? 0 : -1;
        fresh = false;
        pxSinceItem = 0;
        if (visible) videos++;
    }

    long videos; // since the last flush to storage

    private int lastY = -1, swipeStart = -1, maxY = -1, page;
    private int pending; // length of a swipe from the top that may be a page, until another swipe confirms it
    private int lastIndex = -1, maxIndex = -1;
    private boolean fresh; // the index list opened on its first item and hasn't moved past it yet
    private long pxSinceItem, lastTime;

    /**
     * Whether an event is a vertical scroll worth counting. Horizontal lists (story trays, image carousels)
     * would scramble the vertical list's position. dx, dy = raw deltas (-1 = not set);
     * maxScrollY = vertical range (0 or -1 for lists that can't scroll vertically).
     */
    static boolean vertical(int fromIndex, int maxScrollY, int dx, int dy) {
        // Nothing moved vertically: a sideways list, or a nested list laid out without scrolling, whose
        // index would scramble the pager's.
        if (dy == 0) return false;
        if (dy == -1 && dx != 0 && dx != -1) return false; // moved sideways only
        return fromIndex >= 0 || maxScrollY > 0;
    }

    static boolean acceptScroll(boolean requirePager, boolean pagerSource, int index, int range, int dx, int dy) {
        return pagerSource || (!requirePager && vertical(index, range, dx, dy));
    }

    /** A known vertical pager can report a useful position even when its delta or range is zero. */
    void onViewerScroll(int index, int y, int dy) {
        if (index >= 0) {
            viewerIndex = index;
            countViewerPage(index);
        }
        // Some sources leave scrollY at zero while supplying real signed deltas.
        if (y > 0) sawAbsoluteOffset = true;
        if (y >= 0 && sawAbsoluteOffset) viewerOffset = y;
        else if (dy != -1) viewerOffset = Math.max(0, viewerOffset + dy);
    }

    private void countViewerPage(int index) {
        if (index > furthestViewerIndex && index - furthestViewerIndex <= MAX_JUMP) {
            videos += index - furthestViewerIndex;
            furthestViewerIndex = index;
        }
    }

    /**
     * fromIndex = first visible item or -1, scrollY = absolute position or -1, dy = signed pixels moved
     * (positive = forward, -1 when unknown), screenH = screen height px, now = monotonic ms.
     */
    void onScroll(int fromIndex, int scrollY, int dy, int screenH, long now) {
        if (lastTime > 0 && now - lastTime > SWIPE_MS) endSwipe();
        lastTime = now;
        if (viewer) {
            onViewerScroll(fromIndex, scrollY, dy);
            return;
        }
        if (fromIndex >= 0) {
            onIndex(fromIndex, dy, screenH);
            return;
        }
        if (scrollY < 0) return;
        if (lastY >= 0 && Math.abs(scrollY - lastY) > 3 * screenH) {
            swipeStart = -1; // switched lists: positions start over
            maxY = -1;
            lastY = -1;
            pending = 0;
        }
        if (swipeStart < 0) swipeStart = lastY >= 0 ? lastY : scrollY;
        // maxY stays unset until a swipe counts: a feed position only seen, never counted, mustn't hide the
        // first pages of a video viewer opened from it.
        lastY = scrollY;
    }

    private void onIndex(int index, int dy, int screenH) {
        boolean moved = index != lastIndex;
        // In one list the first visible item only changes in the direction scrolled, so moving forward onto a
        // lower index (feed post 5, then reel 0 of a reel opened from it) means another list.
        if (lastIndex < 0 || Math.abs(index - lastIndex) > MAX_JUMP
                || moved && dy != -1 && (dy > 0) != (index > lastIndex)) {
            // First event, or switched lists. A pager may report its first swipe already on item 1 (a reel
            // tapped in the feed, swiped once): moving forward onto item i came from i-1 if it covered a page,
            // which the distance check below decides; otherwise this event's distance belongs to item i.
            moved = dy > 0 && index > 0;
            maxIndex = moved ? index - 1 : index;
            fresh = maxIndex == 0;
            pxSinceItem = 0;
        }
        if (dy != -1) pxSinceItem += Math.abs(dy);
        if (index > maxIndex) {
            int n = index - maxIndex;
            // Unknown distance (dy missing) never counts: a feed and a pager look the same then.
            if (pxSinceItem / n >= screenH * PAGE) videos += n + (fresh ? 1 : 0); // + the video it opened on
            fresh = false;
            maxIndex = index;
        }
        if (moved) pxSinceItem = 0;
        lastIndex = index;
    }

    /** Call once scrolling has been quiet for SWIPE_MS; safe to call twice. */
    void endSwipe() {
        if (hasPixelPager()) {
            if (openingPage >= 0)
                countViewerPage(Math.round((float) viewerOffset / viewerHeight) - openingPage);
            return;
        }
        if (viewer) {
            if (viewerOffset < MIN_PAGE) return;
            if (viewerStride == 0) {
                // Some pagers use logical units. Calibrate from a settled page if pixels don't fit.
                int pages = Math.max(1, Math.round((float) viewerOffset / viewerHeight));
                int expected = viewerIndex > 0 ? viewerIndex : pages;
                if (Math.abs((long) viewerOffset - (long) expected * viewerHeight) <= viewerHeight / 5)
                    viewerStride = Math.max(1, viewerOffset / expected);
                else viewerStride = Math.max(1, viewerOffset / Math.max(1, viewerIndex));
            }
            countViewerPage(Math.round((float) viewerOffset / viewerStride));
            return;
        }
        int start = swipeStart, end = lastY;
        swipeStart = -1;
        if (start < 0 || start == end) return; // no movement: keeps a pending swipe from the top
        int previousPending = pending;
        pending = 0; // only the very next movement can confirm a swipe from the top
        if (end <= maxY) return; // nothing new past the furthest counted video
        int d = end - start;
        if (d < MIN_PAGE) return;
        boolean aligned = start % d == 0 && end % d == 0; // a single page from boundary to boundary
        int from = Math.max(start, maxY);
        if (page == 0 || (d % page != 0 && aligned)) {
            if (!aligned) return; // a feed fling
            // 0 is a boundary for every size, so any feed swipe from the top looks aligned: wait for a second one.
            if (start == 0) {
                pending = d;
                return;
            }
            // Continuing from where the swipe from the top ended: that one was a real page too.
            if (previousPending == d && start == d) from = Math.max(0, maxY);
            page = d; // learn, or relearn if a lucky feed fling taught a wrong size
        }
        if (d % page != 0 || start % page != 0 || d / page > MAX_PAGES) return;
        // Counting from 0 = leaving the pager's first video, which was watched too.
        videos += (end - from) / page + (from == 0 ? 1 : 0);
        maxY = end;
    }
}
