# Analytics
**Lightweight, privacy preserving, analytics for your website(s).**

This project provides a simple, lightweight, and privacy preserving analytics solution for your website(s).
It is written in Rust, designed to be self-hosted at extremely low cost, provides a simple tracking API for
view counts, and ensures that your users' privacy is respected by keeping the data collected as simple as
possible (just the number of times the page is viewed and/or liked).

## Features
- **Counts Views** - Track the number of times a page has been viewed on your website, and display that count
  to your users.

- **Page Likes** - Allow your users to like a page, and track the number of likes that page has received.

- **Privacy Focused** - We don't track any Personally Identifiable Information (PII) about your users,
  just the number of times a page has been viewed and/or liked.

- **Self-Hosted** - Run your own analytics server, ensuring that your data is kept private and secure.

- **Low Cost** - Designed to be run on a small server, with minimal resource requirements through the use
  of Rust and SQLite.

## Usage
To run an instance of the analytics server, you should download the latest release from the
[Releases](https://github.com/SierraSoftworks/analytics/releases) page which corresponds to your platform.
You can then launch the server using the following command.

```bash
# Start the analytics server on port 8080, using the analytics.db database file
./analytics --port 8080 --database analytics.db
```

The server provides a simple HTTP API which can be used to track page views on your website. The easiest
way to do so is to include the following snippet in your website's HTML. This will automatically attach
a 1x1 pixel GIF image to the page which will be loaded by the user's browser, triggering a page view event
on the analytics server.

```html
<script async>
  const trackingImage = document.createElement("img")
  trackingImage.src = `https://$your-analytics-server/embed/${window.location.hostname}/${window.location.pathname}`;
  trackingImage.style.display = "none";
  document.body.appendChild(trackingImage);
</script>
```
