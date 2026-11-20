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
        swipe(c, 1000, 0);                 // first event only sets the position: the first video
        swipe(c, 5000, 1200, 2300);
        swipe(c, 9000, 3500, 4600);
        c.endSwipe();                      // a second call doesn't double count
        assert c.videos == 3 : c.videos;

        // Swiping back and forward again doesn't recount; new videos past the furthest do.
        swipe(c, 13000, 3500, 2300);
        swipe(c, 17000, 3500, 4600);
        assert c.videos == 3 : c.videos;
        swipe(c, 21000, 5800, 6900);
        assert c.videos == 4 : c.videos;

        // A fast fling over two videos counts both, once the page size is known.
        swipe(c, 25000, 8000, 9200, 11500);
        assert c.videos == 6 : c.videos;

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

        // An uncounted feed position can't hide the first confirmed viewer pages.
        c = new ScrollCounter();
        swipe(c, 1000, 3000);
        swipe(c, 2000, 0);
        assert c.videos == 0 : c.videos;
        swipe(c, 3000, 180, 500);
        assert c.videos == 0 : c.videos;
        swipe(c, 4000, 720, 1000);
        assert c.videos == 3 : c.videos;

        // Returning to the top and repeating the same feed swipe isn't confirmation.
        c = new ScrollCounter();
        swipe(c, 1000, 0);
        swipe(c, 2000, 500);
        swipe(c, 3000, 0);
        swipe(c, 4000, 500);
        assert c.videos == 0 : c.videos;

        // A deep-feed jump to mid-swipe gives no evidence of an opening position.
        c = new ScrollCounter();
        swipe(c, 1000, 24080);
        swipe(c, 2000, 180, 500);
        assert c.videos == 0 : c.videos;
        swipe(c, 3000, 720, 1000);
        assert c.videos == 1 : c.videos;

        // A counted furthest page still protects pages already seen.
        c = new ScrollCounter();
        swipe(c, 1000, 1000);
        swipe(c, 2000, 1500);
        swipe(c, 3000, 0);
        swipe(c, 4000, 500);
        swipe(c, 5000, 1000);
        assert c.videos == 1 : c.videos;

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

        // Index pager (Reels, Shorts): each new item after a full screen of scrolling is a video,
        // plus the one it opened on.
        c = new ScrollCounter();
        c.onScroll(0, -1, 50, H, 1000);
        c.onScroll(0, -1, 1200, H, 1100);
        c.onScroll(1, -1, 1150, H, 1200);
        c.onScroll(1, -1, 1300, H, 3000);
        c.onScroll(2, -1, 1100, H, 3100);
        assert c.videos == 3 : c.videos;
        c.onScroll(1, -1, -2400, H, 5000); // back to a seen reel
        c.onScroll(2, -1, 2400, H, 7000);  // and forward again: not new
        assert c.videos == 3 : c.videos;

        // A reel tapped in the home feed opens another list at 0: the tapped reel and every swipe count,
        // though the feed had reached post 6.
        c = new ScrollCounter();
        for (int i = 0; i <= 6; i++) c.onScroll(i, -1, 900, H, 1000 + 100 * i);
        c.onScroll(0, -1, 900, H, 5000);   // first swipe in the reel viewer
        c.onScroll(0, -1, 900, H, 5100);
        c.onScroll(1, -1, 700, H, 5200);
        assert c.videos == 2 : c.videos;
        c.onScroll(1, -1, 1200, H, 7000);
        c.onScroll(2, -1, 1200, H, 7100);
        assert c.videos == 3 : c.videos;
        // A viewer whose first swipe already reports item 1: the tapped reel and that swipe still count.
        ScrollCounter v = new ScrollCounter();
        for (int i = 0; i <= 6; i++) v.onScroll(i, -1, 900, H, 1000 + 100 * i);
        v.onScroll(1, -1, 2300, H, 5000);
        assert v.videos == 2 : v.videos;
        v.onScroll(2, -1, 2300, H, 7000);
        assert v.videos == 3 : v.videos;
        // Back in the feed (still at post 6), scrolling it counts nothing, and a reel opened from it counts again.
        c.onScroll(6, -1, 900, H, 9000);
        c.onScroll(7, -1, 900, H, 9100);
        c.onScroll(0, -1, 1200, H, 12000);
        c.onScroll(1, -1, 1200, H, 12100);
        assert c.videos == 5 : c.videos;

        // Index feed (Instagram home): shorter posts and unknown distances don't count.
        c = new ScrollCounter();
        c.onScroll(0, -1, 400, H, 1000);
        c.onScroll(1, -1, 900, H, 1100);
        c.onScroll(2, -1, 1300, H, 1200);
        c.onScroll(3, -1, -1, H, 1300);
        c.onScroll(4, -1, 0, H, 1400);
        assert c.videos == 0 : c.videos;

        // X opens a full-screen vertical pager from deep in the feed, without scrolling.
        c = new ScrollCounter();
        swipe(c, 1000, 24080);
        c.setViewer(2300);
        c.setViewer(2300);
        assert c.videos == 1 : c.videos;
        c.videos = 0; // service flushes the opened video immediately
        swipe(c, 2000, 180, 2300);
        assert c.videos == 1 : c.videos;
        swipe(c, 3000, 4600, 6901); // fast swipes, one quiet period
        assert c.videos == 3 : c.videos;
        swipe(c, 4000, 4600);
        swipe(c, 5000, 6900);
        assert c.videos == 3 : c.videos;
        c.setViewer(0);
        swipe(c, 6000, 24080, 24327);
        c.setViewer(2300);
        swipe(c, 7000, 500); // pseudo offsets in place of screen pixels
        swipe(c, 8000, 1000);
        assert c.videos == 6 : c.videos;

        // A pager can keep fromIndex at zero and omit deltas while the absolute position advances.
        c = new ScrollCounter();
        c.setViewer(2300);
        c.onScroll(0, 2248, 0, H, 1000);
        c.endSwipe();
        c.onScroll(0, 4504, 0, H, 2000);
        c.endSwipe();
        assert c.videos == 3 : c.videos;
        c.onViewerScroll(2, -1, -1); // selection and scroll for the same landing
        assert c.videos == 3 : c.videos;
        c.onViewerScroll(3, -1, -1); // selection alone
        assert c.videos == 4 : c.videos;
        c.setViewer(0);
        c.setViewer(2300);
        c.onScroll(-1, -1, 1500, H, 3000);
        c.onScroll(-1, -1, 800, H, 3030);
        c.endSwipe();
        assert c.videos == 6 : c.videos;

        // scrollY may stay at zero even when each event reports a vertical movement.
        c = new ScrollCounter();
        c.setViewer(2300);
        c.onScroll(0, 0, 1100, H, 1000);
        c.onScroll(0, 0, 1200, H, 1050);
        c.endSwipe();
        c.onScroll(0, 0, -2300, H, 2000);
        c.endSwipe();
        c.onScroll(0, 0, 2300, H, 3000);
        c.endSwipe();
        assert c.videos == 2 : c.videos;

        // Android 36 X trace: the viewer was still on pages 4 and 8 after foreground changes.
        ScrollCounter.AppSessions sessions = new ScrollCounter.AppSessions();
        c = sessions.enter(true);
        c.setViewer(H);
        for (int landing : new int[]{2408, 4816, 7224, 4816, 7224, 9632})
            swipe(c, 1000, landing);
        assert c.videos == 5 : c.videos;
        c.videos = 0;
        ScrollCounter other = sessions.enter(false);
        assert !other.inViewer() && other.videos == 0;
        c = sessions.enter(true);
        c.setViewer(H);
        assert c.videos == 0 : c.videos;
        swipe(c, 3000, 9685, 10954, 11952, 12037, 12040);
        assert c.videos == 1 : c.videos;
        for (int landing : new int[]{14448, 16856, 19264}) swipe(c, 5000, landing);
        assert c.videos == 4 : c.videos;
        c.videos = 0;
        sessions.enter(false);
        c = sessions.enter(true);
        c.setViewer(H);
        swipe(c, 7000, 19338, 20950, 21620, 21669, 21672);
        assert c.videos == 1 : c.videos;
        swipe(c, 9000, 21682, 22682, 23949, 24072, 24080);
        assert c.videos == 2 : c.videos;
        c.setViewer(0); // returned to X Home, then actually opened another video
        c.setViewer(H);
        assert c.videos == 3 : c.videos;

        // Android 36: opening from Home on page 3, then opening again on page 4.
        c = new ScrollCounter();
        for (int[] positions : new int[][]{{7304, 8893, 9578, 9629, 9632},
                {9732, 11320, 11988, 12037, 12040}}) {
            c.setViewer(H);
            assert c.videos == 1;
            for (int y : positions) assert c.onPagerPosition(y, 38528);
            c.endSwipe();
            assert c.videos == 2 : c.videos;
            c.setViewer(0);
            c.videos = 0;
        }
        // First movement can go backward; revisiting the opening page adds nothing.
        c.setViewer(H);
        for (int y : new int[]{9560, 8100, 7224, 9632}) {
            assert c.onPagerPosition(y, 38528);
            c.endSwipe();
        }
        assert c.videos == 1 : c.videos;
        assert !c.onPagerPosition(65, 65);
        assert c.onPagerPosition(12040, 38528);
        c.endSwipe();
        assert c.videos == 2;

        // Once X exposes its pager, comment-list events cannot seed a swipe before or between videos.
        assert !ScrollCounter.acceptScroll(true, false, -1, 600, -1, -1);
        assert !ScrollCounter.acceptScroll(true, false, 3, -1, 0, H);
        assert ScrollCounter.acceptScroll(true, true, -1, 28896, -1, -1);
        assert ScrollCounter.acceptScroll(false, false, -1, 600, -1, -1);

        assert ScrollCounter.isBottomEditor(850, 2150, 2300, 1080, H);
        assert !ScrollCounter.isBottomEditor(850, 80, 220, 1080, H); // feed search box

        // A nested list is ignored; returning to the same video preserves its settled position.
        c = new ScrollCounter();
        c.setViewer(H);
        c.onPagerPosition(16985, 28896);
        c.onPagerPosition(19264, 28896);
        c.endSwipe();
        assert c.videos == 2;
        assert !ScrollCounter.acceptScroll(true, false, -1, 837, -1, -1);
        c.setViewer(H);
        for (int y : new int[]{19259, 19243, 19261, 19263, 19264}) c.onPagerPosition(y, 52976);
        c.endSwipe();
        assert c.videos == 2;
        c.onPagerPosition(21672, 52976);
        c.endSwipe();
        assert c.videos == 3;

        // Only vertical lists are counted.
        assert ScrollCounter.vertical(3, -1, 0, 1200);        // Reels pager
        assert ScrollCounter.vertical(-1, 60200, -1, -1);     // X feed (Compose, no deltas)
        assert !ScrollCounter.vertical(2, -1, 400, 0);        // story tray / image carousel
        assert !ScrollCounter.vertical(0, -1, 0, 0);          // nested list laid out, not scrolled
        assert !ScrollCounter.vertical(-1, 0, -1, -1);        // Compose horizontal list
        assert !ScrollCounter.vertical(-1, -1, -1, -1);       // nothing to go on

        System.out.println("ScrollCounterTest ok");
    }
}
