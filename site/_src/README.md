# The source of the site

The pages in `site/` are written by `build.mjs` from the fragments in `pages/`: it adds the shared head, header and footer, and writes `robots.txt`, `sitemap.xml` and `img/mark.svg`.
Edit a fragment, then run `node site/_src/build.mjs` from the repository's root, and `node site/_src/check.mjs` to check every link and picture.
The stylesheet (`site/css/site.css`) and the script (`site/js/site.js`) are edited directly. This folder is not served (see `site/.htaccess`).
