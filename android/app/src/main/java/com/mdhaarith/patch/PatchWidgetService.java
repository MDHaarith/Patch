package com.mdhaarith.patch;

import android.content.Context;
import android.content.Intent;
import android.graphics.Color;
import android.text.SpannableString;
import android.text.Spanned;
import android.text.style.StrikethroughSpan;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * Rows of the widget's list, from the snapshot {@link PatchWidget} stored:
 * today's tasks, then an "Upcoming" label and the upcoming ones. Rust has
 * already chosen and ordered them; nothing is filtered here.
 */
public final class PatchWidgetService extends RemoteViewsService {
    @Override
    public RemoteViewsFactory onGetViewFactory(Intent intent) {
        return new Rows(getApplicationContext());
    }

    /** A list row: a section label ({@code task == null}) or a task. */
    private static final class Row {
        final String label;
        final JSONObject task;

        Row(String label, JSONObject task) {
            this.label = label;
            this.task = task;
        }
    }

    private static final class Rows implements RemoteViewsFactory {
        private final Context context;
        private final List<Row> rows = new ArrayList<>();

        Rows(Context context) {
            this.context = context;
        }

        @Override
        public void onCreate() {}

        /** Called on a binder thread after notifyAppWidgetViewDataChanged. */
        @Override
        public void onDataSetChanged() {
            rows.clear();
            JSONObject snap = PatchWidget.snapshot(context);
            JSONArray today = snap.optJSONArray("today");
            JSONArray upcoming = snap.optJSONArray("upcoming");
            int todayCount = today == null ? 0 : today.length();
            int upcomingCount = upcoming == null ? 0 : upcoming.length();
            if (todayCount + upcomingCount == 0) {
                return; // the empty view shows instead
            }
            if (todayCount == 0) {
                rows.add(new Row(context.getString(R.string.widget_nothing_today), null));
            }
            for (int i = 0; i < todayCount; i++) {
                addTask(today.optJSONObject(i));
            }
            if (upcomingCount > 0) {
                rows.add(new Row(context.getString(R.string.widget_upcoming), null));
                for (int i = 0; i < upcomingCount; i++) {
                    addTask(upcoming.optJSONObject(i));
                }
            }
        }

        private void addTask(JSONObject task) {
            if (task != null) {
                rows.add(new Row(null, task));
            }
        }

        @Override
        public void onDestroy() {
            rows.clear();
        }

        @Override
        public int getCount() {
            return rows.size();
        }

        @Override
        public RemoteViews getViewAt(int position) {
            if (position < 0 || position >= rows.size()) {
                return null;
            }
            Row row = rows.get(position);
            if (row.task == null) {
                RemoteViews v = new RemoteViews(context.getPackageName(), R.layout.widget_section);
                v.setTextViewText(R.id.widget_section, row.label);
                return v;
            }
            JSONObject t = row.task;
            boolean done = t.optBoolean("done");
            boolean overdue = t.optBoolean("overdue") && !done;
            RemoteViews v = new RemoteViews(context.getPackageName(), R.layout.widget_item);

            String title = t.optString("title");
            if (done) {
                SpannableString struck = new SpannableString(title);
                struck.setSpan(new StrikethroughSpan(), 0, title.length(), Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);
                v.setTextViewText(R.id.widget_title, struck);
            } else {
                v.setTextViewText(R.id.widget_title, title);
            }
            v.setTextColor(R.id.widget_title, context.getColor(done ? R.color.widget_done : R.color.widget_text));

            v.setTextViewText(R.id.widget_when, t.optString("when"));
            v.setTextColor(R.id.widget_when, context.getColor(
                    overdue ? R.color.widget_overdue : done ? R.color.widget_done : R.color.widget_secondary));

            v.setInt(R.id.widget_dot, "setColorFilter", color(t.optString("color")));
            v.setInt(R.id.widget_dot, "setImageAlpha", done ? 90 : 255);

            String id = t.optString("id");
            if (!id.isEmpty()) {
                v.setOnClickFillInIntent(R.id.widget_row, new Intent().putExtra(MainActivity.EXTRA_TASK_ID, id));
            }
            return v;
        }

        private int color(String hex) {
            try {
                return Color.parseColor(hex);
            } catch (IllegalArgumentException | StringIndexOutOfBoundsException e) {
                return context.getColor(R.color.accent);
            }
        }

        @Override
        public RemoteViews getLoadingView() {
            return null; // the default
        }

        @Override
        public int getViewTypeCount() {
            return 2;
        }

        @Override
        public long getItemId(int position) {
            return position;
        }

        @Override
        public boolean hasStableIds() {
            return false;
        }
    }
}
