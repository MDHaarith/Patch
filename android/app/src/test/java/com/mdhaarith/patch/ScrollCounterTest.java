package com.mdhaarith.patch;

/** Standalone check: run with -ea. */
public final class ScrollCounterTest {
    private static final int H = 2408;

    /** A position-only (Compose/ViewPager) swipe: events at each y, then the swipe ends. */
    private static void swipe(ScrollCounter c, long t, int... ys) {
        for (int y : ys) c.onScroll(-1, y, -1, H, t += 50);
        c.endSwipe();
    }

    public static void main(String[] args) {
        // Video pager: each swipe moves one page (2300 px) from boundary to boundary.
        ScrollCounter c = new ScrollCounter();
        swipe(c, 1000, 0);                 // first event only sets the position
        swipe(c, 5000, 1200, 2300);
        swipe(c, 9000, 3500, 4600);
        c.endSwipe();                      // a second call doesn't double count
        assert c.videos == 2 : c.videos;

        // Swiping back and forward again doesn't recount; new videos past the furthest do.
        swipe(c, 13000, 3500, 2300);
        swipe(c, 17000, 3500, 4600);
        assert c.videos == 2 : c.videos;
        swipe(c, 21000, 5800, 6900);
        assert c.videos == 3 : c.videos;

        // A fast fling over two videos counts both, once the page size is known.
        swipe(c, 25000, 8000, 9200, 11500);
        assert c.videos == 5 : c.videos;

        // Swipe end detected by the next event after a quiet gap.
        c = new ScrollCounter();
        c.onScroll(-1, 2300, -1, H, 1000);
        c.onScroll(-1, 4600, -1, H, 1100);
        c.onScroll(-1, 5000, -1, H, 3000);
        assert c.videos == 1 : c.videos;

        // A pager reporting Compose's pseudo offset (500 per page) still counts.
        c = new ScrollCounter();
        swipe(c, 1000, 1000);
        swipe(c, 2000, 1300, 1500);
        assert c.videos == 1 : c.videos;

        // Feed flings (Compose pseudo offsets, from a real X 12.44 log) don't count.
        c = new ScrollCounter();
        swipe(c, 1000, 21730, 22567, 23989, 24077, 24080);
        swipe(c, 5000, 31373, 32660, 33679, 33711, 33712);
        assert c.videos == 0 : c.videos;

        // Feed swipes from the top (0 fits every page size) don't count, even after coming back to the top.
        c = new ScrollCounter();
        swipe(c, 1000, 0);
        swipe(c, 2000, 870);
        swipe(c, 3000, 0);
        swipe(c, 4000, 640);
        assert c.videos == 0 : c.videos;

        // After the page size is known, a feed fling of another length doesn't count.
        c = new ScrollCounter();
        swipe(c, 1000, 2300);
        swipe(c, 2000, 4600);
        swipe(c, 3000, 5400, 5717);
        assert c.videos == 1 : c.videos;

        // A lucky aligned feed fling teaching a wrong page size is corrected by the next real page.
        c = new ScrollCounter();
        swipe(c, 1000, 3000);
        swipe(c, 2000, 4000);              // aligned by chance: learns 1000, counts 1
        swipe(c, 3000, 4600);              // not aligned: nothing
        swipe(c, 4000, 6900);              // real 2300-px page: relearned and counted
        assert c.videos == 2 : c.videos;

        // Switching lists or scrolling back isn't a video.
        c = new ScrollCounter();
        swipe(c, 1000, 2300, 40000);
        swipe(c, 2000, 37700);
        assert c.videos == 0 : c.videos;

        // Index pager (Reels, Shorts): each new item after a full screen of scrolling is a video.
        c = new ScrollCounter();
        c.onScroll(0, -1, 50, H, 1000);
        c.onScroll(0, -1, 1200, H, 1100);
        c.onScroll(1, -1, 1150, H, 1200);
        c.onScroll(1, -1, 1300, H, 3000);
        c.onScroll(2, -1, 1100, H, 3100);
        assert c.videos == 2 : c.videos;
        c.onScroll(1, -1, 2400, H, 5000);  // back to a seen reel
        c.onScroll(2, -1, 2400, H, 7000);  // and forward again: not new
        assert c.videos == 2 : c.videos;

        // Index feed (Instagram home): shorter posts and unknown distances don't count.
        c = new ScrollCounter();
        c.onScroll(0, -1, 400, H, 1000);
        c.onScroll(1, -1, 900, H, 1100);
        c.onScroll(2, -1, 1300, H, 1200);
        c.onScroll(3, -1, -1, H, 1300);
        c.onScroll(4, -1, 0, H, 1400);
        assert c.videos == 0 : c.videos;

        // Only vertical lists are counted.
        assert ScrollCounter.vertical(3, -1, 0, 1200);        // Reels pager
        assert ScrollCounter.vertical(-1, 60200, -1, -1);     // X feed (Compose, no deltas)
        assert !ScrollCounter.vertical(2, -1, 400, 0);        // story tray / image carousel
        assert !ScrollCounter.vertical(-1, 0, -1, -1);        // Compose horizontal list
        assert !ScrollCounter.vertical(-1, -1, -1, -1);       // nothing to go on

        System.out.println("ScrollCounterTest ok");
    }
}
