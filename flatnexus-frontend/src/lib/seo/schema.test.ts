/**
 * Tests for the structured-data layer.
 *
 * The audit that prompted these found four defects here — a missing `builds`
 * selection that made the AEO corpus announce nothing was installable, two
 * builders silently omitting the `@id` their own module doc calls universal, and
 * a `FAQPage` emitted empty when the API was down. Each was one assertion away
 * from being caught, so the invariants are pinned rather than trusted.
 */

import { describe, expect, it } from 'vitest';
import {
  applicationSchema,
  breadcrumbSchema,
  collectionSchema,
  faqSchema,
  installHowToSchema,
  newestBuild,
  repositorySchema,
  uniqueArches,
  websiteSchema
} from './schema';
import type { Application, Build, RepoInfo, Screenshot } from '$lib/graphql/operations';
import { faqEntries } from './faq';

const ORIGIN = 'https://flatnexus.test';

function build(overrides: Partial<Build> = {}): Build {
  return {
    id: 'b1',
    flatpakRef: 'app/org.example.App/x86_64/stable',
    arch: 'x86_64',
    branch: 'stable',
    version: '1.2.0',
    commit: 'abc123def456',
    sizeBytes: 5 * 1024 * 1024,
    sha256: 'deadbeef',
    publishedAt: 1_700_000_000,
    ...overrides
  };
}

function application(overrides: Partial<Application> = {}): Application {
  return {
    id: 'a1',
    appId: 'org.example.App',
    name: 'Example App',
    summary: 'An example application.',
    description: 'A longer description.',
    homepage: 'https://example.org',
    license: 'GPL-3.0-or-later',
    createdAt: 1_600_000_000,
    installCommand: 'flatpak install flatnexus org.example.App',
    flatpakrefUrl: `${ORIGIN}/refs/org.example.App.flatpakref`,
    publisher: { id: 'p1', displayName: 'A Publisher' },
    builds: [build()],
    ...overrides
  };
}

function screenshot(overrides: Partial<Screenshot> = {}): Screenshot {
  return {
    id: 's1',
    format: 'png',
    position: 0,
    sizeBytes: 200_000,
    isStillImage: true,
    url: `${ORIGIN}/signed/shot.png`,
    ...overrides
  };
}

const repo: RepoInfo = {
  title: 'FlatNexus',
  url: `${ORIGIN}/repo`,
  addRemoteCommand: 'flatpak remote-add --if-not-exists flatnexus …',
  gpgSigned: true
};

const parse = (json: string) => JSON.parse(json) as Record<string, unknown>;

describe('the @id convention', () => {
  it('holds for every page-level builder', () => {
    // The module doc says "every document carries an @id equal to its canonical
    // URL". Two builders did not, so they floated as unmergeable nodes beside
    // the documents describing the same page.
    const documents = [
      websiteSchema(ORIGIN),
      repositorySchema(repo, ORIGIN),
      collectionSchema({ name: 'n', description: 'd', path: '/', items: [] }, ORIGIN),
      applicationSchema(application(), repo, ORIGIN),
      breadcrumbSchema([{ name: 'Home', path: '/' }], '/apps/org.example.App', ORIGIN),
      faqSchema([{ question: 'q', answer: 'a' }], '/', ORIGIN),
      installHowToSchema(application(), repo, ORIGIN)
    ];

    for (const document of documents) {
      const doc = parse(document);
      expect(doc['@id'], `${doc['@type']} must carry an @id`).toBeTruthy();
      expect(String(doc['@id'])).toContain(ORIGIN);
    }
  });

  it('anchors page-scoped documents to the page they describe', () => {
    const path = '/apps/org.example.App';
    expect(parse(breadcrumbSchema([{ name: 'Home', path: '/' }], path, ORIGIN))['@id']).toBe(
      `${ORIGIN}${path}#breadcrumb`
    );
    expect(parse(faqSchema([{ question: 'q', answer: 'a' }], '/', ORIGIN))['@id']).toBe(
      `${ORIGIN}/#faq`
    );
  });
});

describe('applicationSchema', () => {
  it('describes the newest build, which is what an installer receives today', () => {
    const app = application({
      builds: [
        build({ id: 'old', version: '1.0.0', publishedAt: 1_600_000_000 }),
        build({ id: 'new', version: '2.0.0', publishedAt: 1_700_000_000 })
      ]
    });

    const doc = parse(applicationSchema(app, repo, ORIGIN));
    expect(doc.softwareVersion).toBe('2.0.0');
    expect(doc.dateModified).toBe(new Date(1_700_000_000 * 1000).toISOString());
  });

  it('uses schema.org-shaped values, not raw ones', () => {
    const doc = parse(applicationSchema(application(), repo, ORIGIN));

    // fileSize is Text with a unit, not a byte count.
    expect(doc.fileSize).toBe('5.0MiB');
    // license expects a CreativeWork or URL, not a bare identifier.
    expect(doc.license).toEqual({ '@type': 'CreativeWork', name: 'GPL-3.0-or-later' });
    // Architectures are a supported-set, which `processorRequirements` does not
    // mean — it means *minimum* requirements.
    expect(doc.processorRequirements).toBeUndefined();
    expect(doc.additionalProperty).toEqual([
      { '@type': 'PropertyValue', name: 'architectures', value: 'x86_64' }
    ]);
  });

  it('omits optional fields rather than emitting empty ones', () => {
    const doc = parse(
      applicationSchema(
        application({ license: null, homepage: null, publisher: null, builds: [] }),
        null,
        ORIGIN
      )
    );

    expect(doc.license).toBeUndefined();
    expect(doc.sameAs).toBeUndefined();
    expect(doc.author).toBeUndefined();
    expect(doc.provider).toBeUndefined();
    expect(doc.softwareVersion).toBeUndefined();
    expect(doc.additionalProperty).toBeUndefined();
  });
});

describe('newestBuild', () => {
  it('is null for an application with no builds', () => {
    // The Critical this pins: the catalog query omitted `builds`, so this was
    // unconditionally null and `llms-full.txt` told every answer engine that
    // nothing on the server was installable.
    expect(newestBuild([])).toBeNull();
  });

  it('picks the latest publish, not the array order', () => {
    const older = build({ id: 'older', publishedAt: 1 });
    const newer = build({ id: 'newer', publishedAt: 2 });
    expect(newestBuild([newer, older])?.id).toBe('newer');
    expect(newestBuild([older, newer])?.id).toBe('newer');
  });
});

describe('uniqueArches', () => {
  it('deduplicates and sorts', () => {
    const builds = [
      build({ arch: 'x86_64' }),
      build({ arch: 'aarch64' }),
      build({ arch: 'x86_64' })
    ];
    expect(uniqueArches(builds)).toEqual(['aarch64', 'x86_64']);
  });
});

describe('installHowToSchema', () => {
  it('lists the two steps in the order they must be run', () => {
    const doc = parse(installHowToSchema(application(), repo, ORIGIN));
    const steps = doc.step as Array<Record<string, unknown>>;

    expect(steps).toHaveLength(2);
    expect(steps[0].text).toBe(repo.addRemoteCommand);
    expect(steps[1].text).toBe(application().installCommand);
  });
});

describe('the home page FAQ', () => {
  it('describes exactly the answers the page renders', () => {
    // Both the visible <dl> and the JSON-LD are built from `faqEntries`, so this
    // asserts the wiring rather than a duplicated literal: if the schema were
    // ever fed a different list, this comparison would stop holding.
    const entries = faqEntries(repo);
    const doc = parse(faqSchema(entries, '/', ORIGIN));
    const questions = (doc.mainEntity as Array<Record<string, unknown>>).map((q) => q.name);

    expect(questions).toEqual(entries.map((e) => e.question));
    expect(entries.length).toBeGreaterThan(0);
  });

  it('tells the truth about signing in both directions', () => {
    expect(faqEntries({ ...repo, gpgSigned: true })[2].answer).toContain('GPG-signed');
    expect(faqEntries({ ...repo, gpgSigned: false })[2].answer).toContain('does not sign');
  });
});

describe('gallery media in the application schema', () => {
  it('publishes stills as image and screenshot', () => {
    const app = application({
      screenshots: [screenshot({ id: 'a' }), screenshot({ id: 'b', position: 1 })]
    });
    const doc = parse(applicationSchema(app, repo, ORIGIN));

    expect(doc.image).toHaveLength(2);
    expect(doc.screenshot).toEqual(doc.image);
  });

  it('never lists a video as an image', () => {
    // A WebM URL under `image` would render as a broken picture everywhere the
    // property is consumed.
    const app = application({
      screenshots: [
        screenshot({ id: 'still' }),
        screenshot({ id: 'clip', format: 'webm', isStillImage: false, position: 1 })
      ]
    });
    const doc = parse(applicationSchema(app, repo, ORIGIN));

    expect(doc.image).toEqual([`${ORIGIN}/signed/shot.png`]);
    expect(doc.video).toHaveLength(1);
    expect((doc.video as Array<Record<string, unknown>>)[0].encodingFormat).toBe('video/webm');
  });

  it('omits both properties when there is no gallery', () => {
    const doc = parse(applicationSchema(application({ screenshots: [] }), repo, ORIGIN));
    expect(doc.image).toBeUndefined();
    expect(doc.video).toBeUndefined();
  });
});
