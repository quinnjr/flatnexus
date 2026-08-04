/**
 * Every GraphQL document the client sends, in one place.
 *
 * Kept as plain strings rather than generated artifacts so the scaffold has no
 * codegen step. The server publishes its SDL at `GET /graphql/schema` if you
 * later want to generate typed documents from it.
 */

const APPLICATION_FIELDS = `
  id
  appId
  name
  summary
  description
  homepage
  license
  createdAt
  installCommand
  flatpakrefUrl
  publisher {
    id
    displayName
  }
`;

/**
 * Build fields that are plain columns.
 *
 * `bundleUrl` is deliberately absent: it is not stored, it is an S3 presign
 * computed per build on every request. Selecting it from a list query costs one
 * signature per build for a value most callers discard.
 */
const BUILD_FIELDS = `
  id
  flatpakRef
  arch
  branch
  version
  commit
  sizeBytes
  sha256
  publishedAt
`;

/** Build fields plus the presigned archive link. Only for pages that render it. */
const BUILD_FIELDS_WITH_BUNDLE = `
  ${BUILD_FIELDS}
  bundleUrl
`;

/**
 * Gallery media.
 *
 * Kept out of `APPLICATION_FIELDS`: `url` is a presigned S3 signature computed
 * per item, so selecting it from a list query costs one signature per screenshot
 * per application. Only the pages that render a gallery ask for it.
 */
const SCREENSHOT_FIELDS = `
  id
  format
  position
  sizeBytes
  isStillImage
  url
`;

const VIEWER_FIELDS = `
  id
  email
  displayName
  isAdmin
  otpMethod
  totpConfirmed
  hasPassword
`;

export const BROWSE_APPLICATIONS = `
  query BrowseApplications($search: String, $limit: Int, $offset: Int) {
    applications(search: $search, limit: $limit, offset: $offset) {
      ${APPLICATION_FIELDS}
    }
  }
`;

/**
 * The catalog document for `sitemap.xml`, `llms.txt`, and `llms-full.txt`.
 *
 * Separate from `BROWSE_APPLICATIONS` for two reasons. It selects `builds`,
 * without which `newestBuild` is unconditionally null — which previously made
 * every sitemap `lastmod` the registration date and made `llms-full.txt` announce
 * that every application on the server had no builds and nothing to install. And
 * it omits `publisher`, a resolver that costs one query per row and that none of
 * these three endpoints renders.
 */
export const CATALOG_APPLICATIONS = `
  query CatalogApplications($limit: Int, $offset: Int) {
    applications(limit: $limit, offset: $offset) {
      id
      appId
      name
      summary
      description
      homepage
      license
      createdAt
      installCommand
      flatpakrefUrl
      builds { ${BUILD_FIELDS} }
    }
  }
`;

export const APPLICATION_DETAIL = `
  query ApplicationDetail($appId: String!) {
    application(appId: $appId) {
      ${APPLICATION_FIELDS}
      screenshots { ${SCREENSHOT_FIELDS} }
      builds { ${BUILD_FIELDS_WITH_BUNDLE} }
    }
    repoInfo {
      title
      url
      addRemoteCommand
      gpgSigned
    }
  }
`;

/**
 * One application for the markdown view.
 *
 * Same shape as `APPLICATION_DETAIL` minus `bundleUrl`, which `applicationMarkdown`
 * never renders — selecting it cost one S3 presign per build on every fetch of a
 * plain text file, and once per application again inside `/llms-full.txt`.
 */
export const APPLICATION_MARKDOWN = `
  query ApplicationMarkdown($appId: String!) {
    application(appId: $appId) {
      ${APPLICATION_FIELDS}
      screenshots { ${SCREENSHOT_FIELDS} }
      builds { ${BUILD_FIELDS} }
    }
    repoInfo {
      title
      url
      addRemoteCommand
      gpgSigned
    }
  }
`;

export const REPO_INFO = `
  query RepoInfo {
    repoInfo {
      title
      description
      url
      flatpakrepoUrl
      addRemoteCommand
      gpgSigned
    }
    oauthProviders {
      id
      displayName
      startUrl
    }
  }
`;

/**
 * Just the sign-in providers.
 *
 * The login and register pages previously issued the whole `REPO_INFO` document
 * to read this one field, pulling six `repoInfo` values they discard.
 */
export const OAUTH_PROVIDERS = `
  query OAuthProviders {
    oauthProviders {
      id
      displayName
      startUrl
    }
  }
`;

export const ME = `
  query Me {
    me { ${VIEWER_FIELDS} }
  }
`;

export const MY_APPLICATIONS = `
  query MyApplications {
    myApplications {
      ${APPLICATION_FIELDS}
      builds { ${BUILD_FIELDS} }
    }
  }
`;

export const REGISTER = `
  mutation Register($input: RegisterInput!) {
    register(input: $input) {
      token
      expiresAt
      viewer { ${VIEWER_FIELDS} }
    }
  }
`;

/**
 * Step one of sign-in. A correct password yields either a session (for an
 * account with no usable second factor) or a challenge to redeem with an OTP.
 */
export const LOGIN = `
  mutation Login($input: LoginInput!) {
    login(input: $input) {
      status
      session {
        token
        expiresAt
        viewer { ${VIEWER_FIELDS} }
      }
      challenge {
        challengeToken
        method
        sentTo
        expiresInSecs
      }
    }
  }
`;

/** Step two: redeem the challenge with the one-time password. */
export const VERIFY_LOGIN = `
  mutation VerifyLogin($challengeToken: String!, $otp: String!) {
    verifyLogin(challengeToken: $challengeToken, otp: $otp) {
      token
      expiresAt
      viewer { ${VIEWER_FIELDS} }
    }
  }
`;

/**
 * Registering an application.
 *
 * Carries files, so it goes over the multipart request spec like `publishBuild`:
 * `input.screenshots` must be `[null, ...]` in the JSON operations part, with
 * each slot named in the `map`.
 */
export const CREATE_APPLICATION = `
  mutation CreateApplication($input: CreateApplicationInput!) {
    createApplication(input: $input) {
      ${APPLICATION_FIELDS}
    }
  }
`;

/** Replace an application's whole gallery. Also multipart. */
export const REPLACE_SCREENSHOTS = `
  mutation ReplaceScreenshots($input: ReplaceScreenshotsInput!) {
    replaceScreenshots(input: $input) {
      ${APPLICATION_FIELDS}
      screenshots { ${SCREENSHOT_FIELDS} }
    }
  }
`;

export const ENROLL_TOTP = `
  mutation EnrollTotp {
    enrollTotp { secret uri }
  }
`;

export const CONFIRM_TOTP = `
  mutation ConfirmTotp($code: String!) {
    confirmTotp(code: $code) { ${VIEWER_FIELDS} }
  }
`;

export const SET_OTP_METHOD = `
  mutation SetOtpMethod($method: OtpMethodGql!) {
    setOtpMethod(method: $method) { ${VIEWER_FIELDS} }
  }
`;

export const REQUEST_UPLOAD_OTP = `
  mutation RequestUploadOtp {
    requestUploadOtp { sentTo expiresInSecs }
  }
`;

/**
 * The bundle upload.
 *
 * `input.bundle` must be sent as `null` in the JSON operations part; the
 * multipart `map` names that path, and the server substitutes the file. See
 * `gqlUpload` in `client.ts`.
 */
export const PUBLISH_BUILD = `
  mutation PublishBuild($input: PublishBuildInput!) {
    publishBuild(input: $input) {
      ${BUILD_FIELDS}
    }
  }
`;

export const DELETE_BUILD = `
  mutation DeleteBuild($id: ID!, $otp: String!) {
    deleteBuild(id: $id, otp: $otp)
  }
`;

export type Application = {
  id: string;
  appId: string;
  name: string;
  summary: string;
  description: string;
  homepage: string | null;
  license: string | null;
  createdAt: number;
  installCommand: string;
  flatpakrefUrl: string;
  publisher: { id: string; displayName: string } | null;
  builds?: Build[];
  screenshots?: Screenshot[];
};

export type Screenshot = {
  id: string;
  /** `png`, `jpg`, `webp`, or `webm` — sniffed server-side, not from the name. */
  format: string;
  position: number;
  sizeBytes: number;
  /** False for a WebM clip, which does not satisfy the screenshot requirement. */
  isStillImage: boolean;
  /** Time-limited signed URL. */
  url: string;
};

export type Build = {
  id: string;
  flatpakRef: string;
  arch: string;
  branch: string;
  version: string;
  commit: string;
  sizeBytes: number;
  sha256: string;
  publishedAt: number;
  /**
   * Time-limited link to the original .flatpak bundle in object storage.
   *
   * The archived artifact, not the install path — installing goes through the
   * OSTree repository. Absent unless the document selected it; only the
   * application detail page does.
   */
  bundleUrl?: string | null;
};

export type Viewer = {
  id: string;
  email: string;
  displayName: string;
  isAdmin: boolean;
  otpMethod: 'TOTP' | 'EMAIL';
  totpConfirmed: boolean;
  hasPassword: boolean;
};

export type AuthPayload = {
  token: string;
  expiresAt: number;
  viewer: Viewer;
};

export type LoginChallenge = {
  challengeToken: string;
  method: 'TOTP' | 'EMAIL';
  /** Masked address for EMAIL; null for TOTP, where nothing was sent. */
  sentTo: string | null;
  expiresInSecs: number;
};

export type LoginResult = {
  status: 'AUTHENTICATED' | 'OTP_REQUIRED';
  session: AuthPayload | null;
  challenge: LoginChallenge | null;
};

export type RepoInfo = {
  title: string;
  description?: string;
  url: string;
  flatpakrepoUrl?: string;
  addRemoteCommand: string;
  gpgSigned: boolean;
};

export type OAuthProvider = {
  id: string;
  displayName: string;
  startUrl: string;
};
