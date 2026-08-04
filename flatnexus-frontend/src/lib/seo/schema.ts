/**
 * JSON-LD builders.
 *
 * Structured data is what makes this site legible to an answer engine: the
 * difference between "some page mentioning org.example.App" and "a
 * SoftwareApplication named X, version Y, installable with this command". Each
 * builder returns serialized JSON so `<Seo />` can drop it straight into a
 * script tag.
 *
 * Every document carries an `@id` equal to its canonical URL, so separate
 * documents about the same thing merge rather than compete.
 */

import type { Application, Build, RepoInfo } from '$lib/graphql/operations';
import { formatBytes } from '$lib/format';
import { SITE_DESCRIPTION, SITE_NAME, absolute, applicationPath } from './site';

type Json = Record<string, unknown>;

const serialize = (doc: Json): string => JSON.stringify(doc);

/** The site itself, with the search action that powers a sitelinks search box. */
export function websiteSchema(origin?: string | URL): string {
  const home = absolute('/', origin);
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'WebSite',
    '@id': home,
    name: SITE_NAME,
    description: SITE_DESCRIPTION,
    url: home,
    inLanguage: 'en',
    potentialAction: {
      '@type': 'SearchAction',
      target: {
        '@type': 'EntryPoint',
        urlTemplate: `${absolute('/', origin)}?search={search_term_string}`
      },
      'query-input': 'required name=search_term_string'
    }
  });
}

/**
 * The repository as a `DataCatalog`.
 *
 * A Flatpak remote is a catalog of installable things rather than a product in
 * its own right, and modelling it as one lets each application be a `DataSet`-
 * like member without claiming the site *is* the software.
 */
export function repositorySchema(repo: RepoInfo, origin?: string | URL): string {
  const home = absolute('/', origin);
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'DataCatalog',
    '@id': `${home}#repository`,
    name: repo.title,
    description: repo.description ?? SITE_DESCRIPTION,
    url: repo.url,
    isPartOf: { '@type': 'WebSite', '@id': home },
    // Surfaces the one fact a security-conscious reader wants: whether what
    // they install from here is signed.
    additionalProperty: [
      {
        '@type': 'PropertyValue',
        name: 'gpgSigned',
        value: repo.gpgSigned
      },
      {
        '@type': 'PropertyValue',
        name: 'addRemoteCommand',
        value: repo.addRemoteCommand
      }
    ]
  });
}

/** An index page plus the ordered set it lists. */
export function collectionSchema(
  input: {
    name: string;
    description: string;
    path: string;
    items: { name: string; path: string }[];
  },
  origin?: string | URL
): string {
  const canonical = absolute(input.path, origin);
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'CollectionPage',
    '@id': canonical,
    name: input.name,
    description: input.description,
    url: canonical,
    inLanguage: 'en',
    isPartOf: { '@type': 'WebSite', '@id': absolute('/', origin) },
    mainEntity: {
      '@type': 'ItemList',
      numberOfItems: input.items.length,
      itemListElement: input.items.map((item, index) => ({
        '@type': 'ListItem',
        position: index + 1,
        name: item.name,
        url: absolute(item.path, origin)
      }))
    }
  });
}

/**
 * A published application as a `SoftwareApplication`.
 *
 * The type that matters most here — it is what lets an answer engine say "yes,
 * that is installable, here is the command" rather than paraphrasing a page.
 * `softwareVersion`, `fileSize`, and `operatingSystem` come from the newest
 * build, since that is what someone installing today actually receives.
 */
export function applicationSchema(app: Application, repo: RepoInfo | null, origin?: string | URL): string {
  const canonical = absolute(applicationPath(app.appId), origin);
  const newest = newestBuild(app.builds ?? []);

  const doc: Json = {
    '@context': 'https://schema.org',
    '@type': 'SoftwareApplication',
    '@id': canonical,
    name: app.name,
    alternateName: app.appId,
    identifier: app.appId,
    description: app.description || app.summary,
    abstract: app.summary,
    url: canonical,
    inLanguage: 'en',
    applicationCategory: 'DesktopApplication',
    operatingSystem: 'Linux',
    // Flatpak is the distribution mechanism, which is the practically useful
    // fact for anyone deciding whether they can install this.
    installUrl: app.flatpakrefUrl,
    downloadUrl: app.flatpakrefUrl,
    softwareHelp: { '@type': 'CreativeWork', text: app.installCommand },
    isAccessibleForFree: true,
    offers: { '@type': 'Offer', price: '0', priceCurrency: 'USD' },
    mainEntityOfPage: { '@type': 'WebPage', '@id': canonical },
    isPartOf: { '@type': 'WebSite', '@id': absolute('/', origin) }
  };

  // `license` expects a CreativeWork or a URL, not a bare identifier string.
  if (app.license) doc.license = { '@type': 'CreativeWork', name: app.license };
  if (app.homepage) doc.sameAs = [app.homepage];
  if (app.publisher) {
    doc.author = { '@type': 'Person', name: app.publisher.displayName };
    doc.publisher = { '@type': 'Organization', name: app.publisher.displayName };
  }
  if (repo) {
    doc.provider = { '@type': 'Organization', name: repo.title, url: repo.url };
  }

  // `image` is what turns a plain result into an illustrated one, and it is the
  // property most likely to be surfaced by an answer engine summarising an app.
  // Only stills: a video URL here would be a broken image everywhere it is used.
  const stills = (app.screenshots ?? []).filter((item) => item.isStillImage);
  if (stills.length > 0) {
    doc.image = stills.map((item) => item.url);
    doc.screenshot = stills.map((item) => item.url);
  }

  const clips = (app.screenshots ?? []).filter((item) => !item.isStillImage);
  if (clips.length > 0) {
    doc.video = clips.map((clip) => ({
      '@type': 'VideoObject',
      name: `${app.name} demonstration`,
      contentUrl: clip.url,
      encodingFormat: 'video/webm'
    }));
  }

  if (newest) {
    if (newest.version) doc.softwareVersion = newest.version;
    // schema.org documents `fileSize` as Text with a unit suffix, not a raw
    // byte count.
    doc.fileSize = formatBytes(newest.sizeBytes).replace(' ', '');
    doc.dateModified = new Date(newest.publishedAt * 1000).toISOString();
  }

  const arches = uniqueArches(app.builds ?? []);
  if (arches.length > 0) {
    // Not `processorRequirements`, which means *minimum* requirements rather
    // than a list of supported architectures. A PropertyValue says what this
    // actually is, the same way `repositorySchema` reports `gpgSigned`.
    doc.additionalProperty = [
      { '@type': 'PropertyValue', name: 'architectures', value: arches.join(', ') }
    ];
  }
  doc.dateCreated = new Date(app.createdAt * 1000).toISOString();

  return serialize(doc);
}

/**
 * A real ancestry trail for the current page, not the site navigation.
 *
 * `path` is the page the trail describes; it becomes the document's `@id` so this
 * node attaches to that page rather than floating unmergeable beside it.
 */
export function breadcrumbSchema(
  crumbs: { name: string; path: string }[],
  path: string,
  origin?: string | URL
): string {
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'BreadcrumbList',
    '@id': `${absolute(path, origin)}#breadcrumb`,
    itemListElement: crumbs.map((crumb, index) => ({
      '@type': 'ListItem',
      position: index + 1,
      name: crumb.name,
      item: absolute(crumb.path, origin)
    }))
  });
}

/**
 * `FAQPage` built from entries the page also renders.
 *
 * Only ever call this with questions that appear in the visible HTML —
 * structured data describing content a reader cannot see is a guidelines
 * violation, and gets the markup ignored rather than rewarded.
 */
export function faqSchema(
  entries: { question: string; answer: string }[],
  path: string,
  origin?: string | URL
): string {
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'FAQPage',
    '@id': `${absolute(path, origin)}#faq`,
    mainEntity: entries.map((entry) => ({
      '@type': 'Question',
      name: entry.question,
      acceptedAnswer: { '@type': 'Answer', text: entry.answer }
    }))
  });
}

/** How-to steps for adding the remote and installing an application. */
export function installHowToSchema(app: Application, repo: RepoInfo, origin?: string | URL): string {
  const canonical = absolute(applicationPath(app.appId), origin);
  return serialize({
    '@context': 'https://schema.org',
    '@type': 'HowTo',
    '@id': `${canonical}#install`,
    name: `Install ${app.name} with Flatpak`,
    description: `Add the ${repo.title} remote and install ${app.appId}.`,
    totalTime: 'PT2M',
    step: [
      {
        '@type': 'HowToStep',
        position: 1,
        name: 'Add the remote',
        text: repo.addRemoteCommand
      },
      {
        '@type': 'HowToStep',
        position: 2,
        name: 'Install the application',
        text: app.installCommand
      }
    ]
  });
}

export function newestBuild(builds: Build[]): Build | null {
  if (builds.length === 0) return null;
  return builds.reduce((newest, build) => (build.publishedAt > newest.publishedAt ? build : newest));
}

export function uniqueArches(builds: Build[]): string[] {
  return [...new Set(builds.map((build) => build.arch))].sort();
}
